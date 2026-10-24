//! Read the native binary object graph without loading or executing editor code.
//!
//! The schema is documented in `docs/native-format.md`. All lengths and table
//! references are checked before allocation, and unknown tokens are errors.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use serde_json::{Map, Number, Value, json};

const MAX_ITEMS: usize = 1_000_000;
const MAX_DEPTH: usize = 128;
const MAX_BYTES: usize = 256 * 1024 * 1024;
const EMPTY_GUID: &str = "00000000000000000000000000000000";

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: String,
    pub properties: Value,
}

#[derive(Debug, Clone)]
pub struct Graph {
    pub version: [i32; 4],
    pub root: String,
    pub package: String,
    pub objects: BTreeMap<String, Node>,
    encoding: Option<Encoding>,
    dictionary_key_tokens: BTreeMap<String, BTreeMap<String, i32>>,
}

impl Graph {
    /// Construct a new graph. Parsed graphs retain additional private encoding
    /// information; clone those graphs when editing an existing native file.
    pub fn new(
        version: [i32; 4],
        root: String,
        package: String,
        objects: BTreeMap<String, Node>,
    ) -> Self {
        Self {
            version,
            root,
            package,
            objects,
            encoding: None,
            dictionary_key_tokens: BTreeMap::new(),
        }
    }

    /// Specify the native key type for additions to a dictionary, including an
    /// empty/new dictionary that has no original key to infer its type from.
    /// `pointer` is a JSON Pointer into the selected object's properties.
    pub fn set_dictionary_key_token(
        &mut self,
        object_id: &str,
        pointer: &str,
        token: i32,
    ) -> Result<()> {
        ensure!(
            matches!(token, 3 | 5 | 8 | 17 | 20 | 21 | 22 | 29),
            "原生字典键标记不受支持：{token}"
        );
        let node = self
            .objects
            .get(object_id)
            .context("原生字典键类型对应的对象不存在")?;
        ensure!(
            node.properties
                .pointer(pointer)
                .is_some_and(Value::is_object),
            "原生字典键类型对应的路径不是字典：{pointer}"
        );
        self.dictionary_key_tokens
            .entry(object_id.to_owned())
            .or_default()
            .insert(pointer.to_owned(), token);
        Ok(())
    }

    /// Copy a native template object, including the original value encodings.
    /// References inside the object retain their source IDs; the caller can
    /// redirect them after copying any referenced style/text objects it needs.
    pub fn copy_node_from(&mut self, source: &Graph, source_id: &str, new_id: &str) -> Result<()> {
        let new_id = normalize_guid(new_id)?;
        ensure!(
            !self.objects.contains_key(&new_id),
            "新的原生对象 ID 已存在"
        );
        let node = source
            .objects
            .get(source_id)
            .context("原生模板对象不存在")?
            .clone();
        if let Some((source_encoding, record)) = source.encoding.as_ref().and_then(|encoding| {
            encoding
                .records
                .get(source_id)
                .map(|record| (encoding, record))
        }) {
            let target = self.encoding.get_or_insert_with(Encoding::empty);
            let value = rebase_layout(&record.value, source_encoding, target)?;
            target.object_order.push(new_id.clone());
            target.records.insert(
                new_id.clone(),
                RecordLayout {
                    header: 0..0,
                    kind: node.kind.clone(),
                    value,
                },
            );
        }
        if let Some(hints) = source.dictionary_key_tokens.get(source_id) {
            self.dictionary_key_tokens
                .insert(new_id.clone(), hints.clone());
        }
        self.objects.insert(new_id, node);
        Ok(())
    }

    pub fn root(&self) -> Option<&Node> {
        self.objects.get(&self.root)
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.objects.get(id)
    }

    pub fn resolve(&self, value: &Value) -> Option<&Node> {
        reference_id(value).and_then(|id| self.objects.get(id))
    }
}

#[derive(Debug, Clone)]
struct Encoding {
    bytes: Arc<Vec<u8>>,
    strings: Vec<Option<String>>,
    guids: Vec<String>,
    root: String,
    package: String,
    context: Range<usize>,
    object_order: Vec<String>,
    records: BTreeMap<String, RecordLayout>,
}

impl Encoding {
    fn empty() -> Self {
        Self {
            bytes: Arc::new(Vec::new()),
            strings: vec![None],
            guids: vec![EMPTY_GUID.to_owned()],
            root: String::new(),
            package: String::new(),
            context: 0..0,
            object_order: Vec::new(),
            records: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct RecordLayout {
    header: Range<usize>,
    kind: String,
    value: ValueLayout,
}

#[derive(Debug, Clone)]
enum ValueLayout {
    Dictionary {
        token: i32,
        entries: Vec<DictionaryEntry>,
    },
    List {
        token: i32,
        items: Vec<ValueLayout>,
    },
    Scalar {
        token: i32,
        span: Range<usize>,
    },
}

#[derive(Debug, Clone)]
struct DictionaryEntry {
    json_key: String,
    key: Value,
    key_layout: ValueLayout,
    value_layout: ValueLayout,
}

/// Get an object or GUID identifier from a parsed value.
pub fn reference_id(value: &Value) -> Option<&str> {
    value.get("$ref").or_else(|| value.get("$guid"))?.as_str()
}

pub fn parse(bytes: &[u8]) -> Result<Graph> {
    ensure!(bytes.len() <= MAX_BYTES, "原生对象包超过 256 MiB 限制");
    Reader {
        bytes,
        cursor: 0,
        strings: vec![None],
        guids: vec![EMPTY_GUID.to_owned()],
        values_read: 0,
    }
    .graph()
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
    strings: Vec<Option<String>>,
    guids: Vec<String>,
    values_read: usize,
}

impl Reader<'_> {
    fn take(&mut self, len: usize) -> Result<&[u8]> {
        let end = self.cursor.checked_add(len).context("原生长度溢出")?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .with_context(|| format!("原生对象包在偏移 {} 处被截断", self.cursor))?;
        self.cursor = end;
        Ok(value)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("fixed-size slice"))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn count(&mut self) -> Result<usize> {
        let value = self.i32()?;
        ensure!(value >= 0, "原生对象包包含负数长度");
        let count = value as usize;
        ensure!(count <= MAX_ITEMS, "原生对象包项目数量超过限制");
        Ok(count)
    }

    fn length(&mut self) -> Result<usize> {
        let value = self.i32()?;
        ensure!(value >= 0, "原生对象包包含负数数据长度");
        let len = value as usize;
        ensure!(len <= MAX_BYTES, "原生对象包数据长度超过限制");
        Ok(len)
    }

    fn expect(&mut self, expected: i32) -> Result<()> {
        let offset = self.cursor;
        let token = self.i32()?;
        ensure!(
            token == expected,
            "原生对象包偏移 {offset}：需要标记 {expected}，实际为 {token}"
        );
        Ok(())
    }

    fn string(&mut self) -> Result<Option<String>> {
        let id = self.i32()?;
        ensure!(id >= 0, "原生字符串引用为负数");
        self.strings
            .get(id as usize)
            .cloned()
            .with_context(|| format!("原生字符串引用 {id} 超出表范围"))
    }

    fn guid(&mut self) -> Result<String> {
        let id = self.i32()?;
        ensure!(id >= 0, "原生 GUID 引用为负数");
        self.guids
            .get(id as usize)
            .cloned()
            .with_context(|| format!("原生 GUID 引用 {id} 超出表范围"))
    }

    fn graph(mut self) -> Result<Graph> {
        self.expect(27)?;
        let version = [self.i32()?, self.i32()?, self.i32()?, self.i32()?];
        let data_version = self.i32()?;
        ensure!(
            data_version == 50001,
            "暂不支持 6.0.1 之前的原生对象包格式（标记 {data_version}）"
        );
        self.expect(31)?;
        let count = self.count()?;
        ensure!(count <= self.bytes.len() / 4, "原生字符串表数量无效");
        self.strings.reserve(count);
        for _ in 0..count {
            let len = self.length()?;
            let text = std::str::from_utf8(self.take(len)?)
                .context("原生字符串表包含无效 UTF-8")?
                .to_owned();
            self.strings.push(Some(text));
        }
        self.expect(30)?;
        let count = self.count()?;
        ensure!(
            count <= (self.bytes.len() - self.cursor) / 16,
            "原生 GUID 表数量无效"
        );
        self.guids.reserve(count);
        for _ in 0..count {
            let b: [u8; 16] = self.array()?;
            // .NET Guid.ToByteArray uses little endian in the first 3 fields.
            self.guids.push(format!(
                "{:08x}{:04x}{:04x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                u32::from_le_bytes(b[0..4].try_into().unwrap()),
                u16::from_le_bytes(b[4..6].try_into().unwrap()),
                u16::from_le_bytes(b[6..8].try_into().unwrap()),
                b[8],
                b[9],
                b[10],
                b[11],
                b[12],
                b[13],
                b[14],
                b[15]
            ));
        }
        let context_start = self.cursor;
        self.expect(26)?;
        self.expect(15)?;
        let root = self.guid()?;
        self.expect(18)?;
        let package = self.guid()?;
        let context_end = self.cursor;
        let mut objects = BTreeMap::new();
        let mut object_order = Vec::new();
        let mut records = BTreeMap::new();
        loop {
            let header_start = self.cursor;
            let token = self.i32()?;
            if token == 28 {
                break;
            }
            ensure!(token == 9, "原生对象定义标记无效：{token}");
            let id = self.guid()?;
            let kind = self.string()?.context("原生对象类型为空")?;
            let header_end = self.cursor;
            let (properties, value_layout) = self.value(0)?;
            ensure!(properties.is_object(), "原生对象属性不是字典");
            object_order.push(id.clone());
            records.insert(
                id.clone(),
                RecordLayout {
                    header: header_start..header_end,
                    kind: kind.clone(),
                    value: value_layout,
                },
            );
            ensure!(
                objects.insert(id, Node { kind, properties }).is_none(),
                "原生对象 ID 重复"
            );
            ensure!(objects.len() <= MAX_ITEMS, "原生对象数量超过限制");
        }
        ensure!(
            self.cursor == self.bytes.len(),
            "原生对象包结束后存在未识别数据"
        );
        ensure!(objects.contains_key(&root), "原生对象包缺少根对象");
        let encoding = Encoding {
            bytes: Arc::new(self.bytes.to_vec()),
            strings: self.strings,
            guids: self.guids,
            root: root.clone(),
            package: package.clone(),
            context: context_start..context_end,
            object_order,
            records,
        };
        Ok(Graph {
            version,
            root,
            package,
            objects,
            encoding: Some(encoding),
            dictionary_key_tokens: BTreeMap::new(),
        })
    }

    fn value(&mut self, depth: usize) -> Result<(Value, ValueLayout)> {
        ensure!(depth < MAX_DEPTH, "原生对象包嵌套层数超过限制");
        self.values_read += 1;
        ensure!(
            self.values_read <= 4 * MAX_ITEMS,
            "原生对象包值数量超过限制"
        );
        let offset = self.cursor;
        let full_token = self.i32()?;
        let token = full_token & 31;
        let value: Result<Value> = match token {
            1 => {
                let count = self.count()?;
                ensure!(
                    count <= (self.bytes.len() - self.cursor) / 8,
                    "原生字典长度无效"
                );
                let mut map = Map::new();
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let (key, key_layout) = self.value(depth + 1)?;
                    let json_key = match &key {
                        Value::String(text) => text.clone(),
                        Value::Null => "null".to_owned(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        value => reference_id(value)
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string()),
                    };
                    let (value, value_layout) = self.value(depth + 1)?;
                    ensure!(
                        map.insert(json_key.clone(), value).is_none(),
                        "原生字典键重复"
                    );
                    entries.push(DictionaryEntry {
                        json_key,
                        key,
                        key_layout,
                        value_layout,
                    });
                }
                return Ok((
                    Value::Object(map),
                    ValueLayout::Dictionary {
                        token: full_token,
                        entries,
                    },
                ));
            }
            2 => {
                let count = self.count()?;
                ensure!(
                    count <= (self.bytes.len() - self.cursor) / 4,
                    "原生列表长度无效"
                );
                let mut values = Vec::with_capacity(count);
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    let (value, layout) = self.value(depth + 1)?;
                    values.push(value);
                    items.push(layout);
                }
                return Ok((
                    Value::Array(values),
                    ValueLayout::List {
                        token: full_token,
                        items,
                    },
                ));
            }
            3 | 20 | 21 | 22 => Ok(json!({"$ref": self.guid()?, "$token": token})),
            16 | 19 => {
                let kind = self.string()?;
                Ok(json!({"$ref": self.guid()?, "$type": kind, "$token": token}))
            }
            17 => Ok(json!({"$guid": self.guid()?})),
            4 => {
                let len = self.length()?;
                let data = self.take(len)?;
                Ok(json!({"$binary": base64::engine::general_purpose::STANDARD.encode(data)}))
            }
            5 => Ok(self.i32()?.into()),
            6 => {
                let n = f64::from_le_bytes(self.array()?);
                Ok(Number::from_f64(n)
                    .map(Value::Number)
                    .unwrap_or(Value::Null))
            }
            7 => {
                let n = f32::from_le_bytes(self.array()?) as f64;
                Ok(Number::from_f64(n)
                    .map(Value::Number)
                    .unwrap_or(Value::Null))
            }
            8 => Ok(self.string()?.map(Value::String).unwrap_or(Value::Null)),
            25 => Ok((self.take(1)?[0] != 0).into()),
            29 => Ok(i64::from_le_bytes(self.array()?).into()),
            _ => bail!("暂不支持原生对象包标记 {token}（偏移 {offset}）"),
        };
        let value = value?;
        Ok((
            value,
            ValueLayout::Scalar {
                token: full_token,
                span: offset..self.cursor,
            },
        ))
    }
}

/// Encode a graph back into its native binary object package.
///
/// Parsed graphs retain original table indices, object/dictionary order,
/// scalar types, token flags, typed keys, and nonfinite floating-point bits.
/// Untouched scalar values are copied verbatim; edits append new table values.
pub fn encode(graph: &Graph) -> Result<Vec<u8>> {
    ensure!(
        graph.objects.contains_key(&graph.root),
        "原生对象包缺少根对象"
    );
    ensure!(graph.objects.len() <= MAX_ITEMS, "原生对象数量超过限制");
    let mut writer = Writer::new(graph.encoding.as_ref());
    if let Some(source) = graph
        .encoding
        .as_ref()
        .filter(|s| s.root == graph.root && s.package == graph.package)
    {
        writer
            .data
            .extend_from_slice(&source.bytes[source.context.clone()]);
    } else {
        writer.int(26);
        writer.int(15);
        writer.guid(&graph.root)?;
        writer.int(18);
        writer.guid(&graph.package)?;
    }
    let mut order: Vec<&str> = Vec::with_capacity(graph.objects.len());
    if let Some(source) = &graph.encoding {
        order.extend(
            source
                .object_order
                .iter()
                .filter(|id| graph.objects.contains_key(*id))
                .map(String::as_str),
        );
    }
    for id in graph.objects.keys() {
        if !graph
            .encoding
            .as_ref()
            .is_some_and(|s| s.records.contains_key(id))
        {
            order.push(id);
        }
    }
    for id in order {
        let node = &graph.objects[id];
        ensure!(node.properties.is_object(), "原生对象属性不是字典");
        let record = graph.encoding.as_ref().and_then(|s| s.records.get(id));
        writer.key_hints = graph.dictionary_key_tokens.get(id);
        writer.pointer.clear();
        if let Some((source, record)) = graph
            .encoding
            .as_ref()
            .zip(record)
            .filter(|(_, r)| r.kind == node.kind && !r.header.is_empty())
        {
            writer
                .data
                .extend_from_slice(&source.bytes[record.header.clone()]);
        } else {
            writer.int(9);
            writer.guid(id)?;
            writer.string(Some(&node.kind))?;
        }
        writer.value(&node.properties, record.map(|r| &r.value), 0)?;
    }
    writer.int(28);
    let mut output = Vec::new();
    push_int(&mut output, 27);
    for component in graph.version {
        push_int(&mut output, component);
    }
    push_int(&mut output, 50001);
    push_int(&mut output, 31);
    push_int(&mut output, checked_count(writer.strings.len() - 1)?);
    for text in writer.strings.iter().skip(1) {
        let text = text.as_deref().context("原生字符串表条目为空")?;
        push_int(&mut output, checked_length(text.len())?);
        output.extend_from_slice(text.as_bytes());
    }
    push_int(&mut output, 30);
    push_int(&mut output, checked_count(writer.guids.len() - 1)?);
    for guid in writer.guids.iter().skip(1) {
        output.extend_from_slice(&guid_bytes(guid)?);
    }
    ensure!(
        output
            .len()
            .checked_add(writer.data.len())
            .is_some_and(|len| len <= MAX_BYTES),
        "原生对象包超过 256 MiB 限制"
    );
    output.extend_from_slice(&writer.data);
    Ok(output)
}

fn push_int(output: &mut Vec<u8>, value: i32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn rebase_layout(
    layout: &ValueLayout,
    source: &Encoding,
    target: &mut Encoding,
) -> Result<ValueLayout> {
    Ok(match layout {
        ValueLayout::Dictionary { token, entries } => {
            let mut rebased = Vec::with_capacity(entries.len());
            for entry in entries {
                rebased.push(DictionaryEntry {
                    json_key: entry.json_key.clone(),
                    key: entry.key.clone(),
                    key_layout: rebase_layout(&entry.key_layout, source, target)?,
                    value_layout: rebase_layout(&entry.value_layout, source, target)?,
                });
            }
            ValueLayout::Dictionary {
                token: *token,
                entries: rebased,
            }
        }
        ValueLayout::List { token, items } => {
            let items = items
                .iter()
                .map(|item| rebase_layout(item, source, target))
                .collect::<Result<Vec<_>>>()?;
            ValueLayout::List {
                token: *token,
                items,
            }
        }
        ValueLayout::Scalar { token, span } => {
            let mut bytes = source
                .bytes
                .get(span.clone())
                .context("原生模板编码范围无效")?
                .to_vec();
            for (offset, is_guid) in match token & 31 {
                3 | 17 | 20 | 21 | 22 => vec![(4, true)],
                8 => vec![(4, false)],
                16 | 19 => vec![(4, false), (8, true)],
                _ => Vec::new(),
            } {
                let old_index = i32::from_le_bytes(
                    bytes
                        .get(offset..offset + 4)
                        .context("原生模板表引用被截断")?
                        .try_into()?,
                );
                let old_index = usize::try_from(old_index).context("原生模板表引用为负数")?;
                let index = if is_guid {
                    let guid = source
                        .guids
                        .get(old_index)
                        .context("原生模板 GUID 引用超出范围")?;
                    if let Some(index) = target.guids.iter().position(|value| value == guid) {
                        index
                    } else {
                        checked_count(target.guids.len())?;
                        target.guids.push(guid.clone());
                        target.guids.len() - 1
                    }
                } else {
                    let string = source
                        .strings
                        .get(old_index)
                        .context("原生模板字符串引用超出范围")?;
                    if let Some(index) = target.strings.iter().position(|value| value == string) {
                        index
                    } else {
                        checked_count(target.strings.len())?;
                        target.strings.push(string.clone());
                        target.strings.len() - 1
                    }
                };
                bytes[offset..offset + 4].copy_from_slice(&(index as i32).to_le_bytes());
            }
            let start = target.bytes.len();
            ensure!(
                start
                    .checked_add(bytes.len())
                    .is_some_and(|length| length <= MAX_BYTES),
                "原生模板编码超过长度限制"
            );
            Arc::make_mut(&mut target.bytes).extend_from_slice(&bytes);
            ValueLayout::Scalar {
                token: *token,
                span: start..target.bytes.len(),
            }
        }
    })
}

fn checked_count(count: usize) -> Result<i32> {
    ensure!(count <= MAX_ITEMS, "原生对象包项目数量超过限制");
    Ok(count as i32)
}

fn checked_length(len: usize) -> Result<i32> {
    ensure!(len <= MAX_BYTES, "原生对象包数据长度超过限制");
    Ok(len as i32)
}

fn normalize_guid(guid: &str) -> Result<String> {
    let value: String = guid
        .chars()
        .filter(|c| !matches!(c, '-' | '{' | '}'))
        .collect();
    ensure!(
        value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "原生 GUID 格式无效：{guid}"
    );
    Ok(value.to_ascii_lowercase())
}

fn guid_bytes(guid: &str) -> Result<[u8; 16]> {
    let guid = normalize_guid(guid)?;
    let mut canonical = [0_u8; 16];
    for (i, value) in canonical.iter_mut().enumerate() {
        *value = u8::from_str_radix(&guid[i * 2..i * 2 + 2], 16).context("原生 GUID 格式无效")?;
    }
    canonical[0..4].reverse();
    canonical[4..6].reverse();
    canonical[6..8].reverse();
    Ok(canonical)
}

struct Writer<'a> {
    data: Vec<u8>,
    strings: Vec<Option<String>>,
    guids: Vec<String>,
    string_ids: HashMap<String, i32>,
    guid_ids: HashMap<String, i32>,
    source: Option<&'a Encoding>,
    values_written: usize,
    key_hints: Option<&'a BTreeMap<String, i32>>,
    pointer: String,
}

impl<'a> Writer<'a> {
    fn new(source: Option<&'a Encoding>) -> Self {
        let strings = source
            .map(|s| s.strings.clone())
            .unwrap_or_else(|| vec![None]);
        let guids = source
            .map(|s| s.guids.clone())
            .unwrap_or_else(|| vec![EMPTY_GUID.to_owned()]);
        let mut string_ids = HashMap::new();
        let mut guid_ids = HashMap::new();
        for (i, text) in strings.iter().enumerate() {
            if let Some(text) = text {
                string_ids.entry(text.clone()).or_insert(i as i32);
            }
        }
        for (i, guid) in guids.iter().enumerate() {
            guid_ids.entry(guid.clone()).or_insert(i as i32);
        }
        Self {
            data: Vec::new(),
            strings,
            guids,
            string_ids,
            guid_ids,
            source,
            values_written: 0,
            key_hints: None,
            pointer: String::new(),
        }
    }

    fn int(&mut self, value: i32) {
        push_int(&mut self.data, value);
    }

    fn string(&mut self, text: Option<&str>) -> Result<()> {
        let Some(text) = text else {
            self.int(0);
            return Ok(());
        };
        let index = if let Some(index) = self.string_ids.get(text) {
            *index
        } else {
            let index = checked_count(self.strings.len())?;
            checked_length(text.len())?;
            self.strings.push(Some(text.to_owned()));
            self.string_ids.insert(text.to_owned(), index);
            index
        };
        self.int(index);
        Ok(())
    }

    fn guid(&mut self, guid: &str) -> Result<()> {
        let guid = normalize_guid(guid)?;
        let index = if let Some(index) = self.guid_ids.get(&guid) {
            *index
        } else {
            let index = checked_count(self.guids.len())?;
            self.guids.push(guid.clone());
            self.guid_ids.insert(guid, index);
            index
        };
        self.int(index);
        Ok(())
    }

    fn value(&mut self, value: &Value, layout: Option<&ValueLayout>, depth: usize) -> Result<()> {
        ensure!(depth < MAX_DEPTH, "原生对象包嵌套层数超过限制");
        self.values_written += 1;
        ensure!(
            self.values_written <= 4 * MAX_ITEMS,
            "原生对象包值数量超过限制"
        );
        ensure!(self.data.len() <= MAX_BYTES, "原生对象包超过 256 MiB 限制");
        match layout {
            Some(ValueLayout::Dictionary { token, entries }) if value.is_object() => {
                let map = value.as_object().unwrap();
                self.int(*token);
                self.int(checked_count(map.len())?);
                for entry in entries {
                    if let Some(value) = map.get(&entry.json_key) {
                        self.value(&entry.key, Some(&entry.key_layout), depth + 1)?;
                        self.child_value(
                            value,
                            Some(&entry.value_layout),
                            depth + 1,
                            &entry.json_key,
                        )?;
                    }
                }
                for (key, value) in map {
                    if !entries.iter().any(|entry| &entry.json_key == key) {
                        self.new_dictionary_key(key, entries, depth + 1)?;
                        self.child_value(value, None, depth + 1, key)?;
                    }
                }
                return Ok(());
            }
            Some(ValueLayout::List { token, items }) if value.is_array() => {
                let array = value.as_array().unwrap();
                self.int(*token);
                self.int(checked_count(array.len())?);
                for (i, value) in array.iter().enumerate() {
                    self.child_value(value, items.get(i), depth + 1, &i.to_string())?;
                }
                return Ok(());
            }
            Some(ValueLayout::Scalar { token, span }) => {
                if self.scalar_unchanged(value, *token, span) {
                    self.data
                        .extend_from_slice(&self.source.unwrap().bytes[span.clone()]);
                    return Ok(());
                }
                if scalar_compatible(value, *token & 31) {
                    return self.scalar(value, *token);
                }
            }
            _ => (),
        }
        if let Some(array) = value.as_array() {
            self.int(2);
            self.int(checked_count(array.len())?);
            for (index, value) in array.iter().enumerate() {
                self.child_value(value, None, depth + 1, &index.to_string())?;
            }
        } else if is_dictionary(value) {
            let map = value.as_object().unwrap();
            self.int(1);
            self.int(checked_count(map.len())?);
            for (key, value) in map {
                self.new_dictionary_key(key, &[], depth + 1)?;
                self.child_value(value, None, depth + 1, key)?;
            }
        } else {
            let token = infer_scalar_token(value)?;
            self.scalar(value, token)?;
        }
        Ok(())
    }

    fn child_value(
        &mut self,
        value: &Value,
        layout: Option<&ValueLayout>,
        depth: usize,
        component: &str,
    ) -> Result<()> {
        let previous_len = self.pointer.len();
        self.pointer.push('/');
        self.pointer
            .push_str(&component.replace('~', "~0").replace('/', "~1"));
        let result = self.value(value, layout, depth);
        self.pointer.truncate(previous_len);
        result
    }

    fn new_dictionary_key(
        &mut self,
        text: &str,
        entries: &[DictionaryEntry],
        depth: usize,
    ) -> Result<()> {
        if let Some(token) = self
            .key_hints
            .and_then(|hints| hints.get(&self.pointer))
            .copied()
        {
            let key = match token {
                17 => json!({"$guid":normalize_guid(text)?}),
                3 | 20 | 21 | 22 => json!({"$ref":normalize_guid(text)?,"$token":token}),
                5 | 29 => Value::from(text.parse::<i64>().context("原生字典键不是有效整数")?),
                8 => Value::String(text.to_owned()),
                _ => bail!("原生字典键标记不受支持：{token}"),
            };
            return self.scalar(&key, token);
        }
        // Preserve the key domain for additions to existing GUID/enum maps.
        let hint = entries.first();
        if let Some(hint) = hint
            && reference_id(&hint.key).is_some()
            && let Ok(guid) = normalize_guid(text)
        {
            let token = match hint.key_layout {
                ValueLayout::Scalar { token, .. } => token,
                _ => 17,
            };
            let key = if token & 31 == 17 {
                json!({"$guid":guid})
            } else {
                json!({"$ref":guid,"$token":token & 31,"$type":hint.key.get("$type").cloned().unwrap_or(Value::Null)})
            };
            return self.value(&key, Some(&hint.key_layout), depth);
        }
        if let Some(hint) = hint
            && hint.key.is_i64()
            && let Ok(number) = text.parse::<i64>()
        {
            return self.value(&Value::from(number), Some(&hint.key_layout), depth);
        }
        self.value(&Value::String(text.to_owned()), None, depth)
    }

    fn scalar_unchanged(&self, value: &Value, token: i32, span: &Range<usize>) -> bool {
        let Some(source) = self.source else {
            return false;
        };
        let Some(bytes) = source.bytes.get(span.clone()) else {
            return false;
        };
        let Some(payload) = bytes.get(4..) else {
            return false;
        };
        let integer = |offset: usize| -> Option<i32> {
            Some(i32::from_le_bytes(
                payload.get(offset..offset + 4)?.try_into().ok()?,
            ))
        };
        let guid = |offset: usize| -> Option<&str> {
            source
                .guids
                .get(usize::try_from(integer(offset)?).ok()?)
                .map(String::as_str)
        };
        let string = |offset: usize| -> Option<Option<&str>> {
            Some(
                source
                    .strings
                    .get(usize::try_from(integer(offset)?).ok()?)?
                    .as_deref(),
            )
        };
        let reference_matches = |id: Option<&str>| {
            value.get("$ref").and_then(Value::as_str) == id
                && value
                    .get("$token")
                    .and_then(Value::as_i64)
                    .is_none_or(|t| t == i64::from(token & 31))
        };
        match token & 31 {
            3 | 20 | 21 | 22 => reference_matches(guid(0)),
            16 | 19 => {
                reference_matches(guid(4))
                    && value.get("$type").and_then(Value::as_str) == string(0).flatten()
            }
            17 => value.get("$guid").and_then(Value::as_str) == guid(0),
            4 => {
                let Some(data) = payload.get(4..) else {
                    return false;
                };
                value
                    .get("$binary")
                    .and_then(Value::as_str)
                    .is_some_and(|text| {
                        base64::engine::general_purpose::STANDARD.encode(data) == text
                    })
            }
            5 => integer(0).is_some_and(|n| {
                value.as_i64() == Some(i64::from(n)) || value.as_f64() == Some(f64::from(n))
            }),
            6 => payload.try_into().ok().is_some_and(|b| {
                let n = f64::from_le_bytes(b);
                if n.is_finite() {
                    value.as_f64().is_some_and(|v| v.to_bits() == n.to_bits())
                } else {
                    value.is_null()
                }
            }),
            7 => payload.try_into().ok().is_some_and(|b| {
                let n = f32::from_le_bytes(b);
                if n.is_finite() {
                    value
                        .as_f64()
                        .is_some_and(|v| v.to_bits() == (n as f64).to_bits())
                } else {
                    value.is_null()
                }
            }),
            8 => string(0).is_some_and(|s| match s {
                Some(s) => value.as_str() == Some(s),
                None => value.is_null(),
            }),
            25 => payload
                .first()
                .is_some_and(|b| value.as_bool() == Some(*b != 0)),
            29 => payload
                .try_into()
                .ok()
                .is_some_and(|b| value.as_i64() == Some(i64::from_le_bytes(b))),
            _ => false,
        }
    }

    fn scalar(&mut self, value: &Value, token: i32) -> Result<()> {
        self.int(token);
        match token & 31 {
            3 | 20 | 21 | 22 => self.guid(
                value
                    .get("$ref")
                    .and_then(Value::as_str)
                    .context("原生对象引用为空")?,
            )?,
            16 | 19 => {
                self.string(value.get("$type").and_then(Value::as_str))?;
                self.guid(
                    value
                        .get("$ref")
                        .and_then(Value::as_str)
                        .context("原生包引用为空")?,
                )?;
            }
            17 => self.guid(
                value
                    .get("$guid")
                    .and_then(Value::as_str)
                    .context("原生 GUID 值为空")?,
            )?,
            4 => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(
                        value
                            .get("$binary")
                            .and_then(Value::as_str)
                            .context("原生二进制数据为空")?,
                    )
                    .context("原生二进制数据不是有效 Base64")?;
                self.int(checked_length(bytes.len())?);
                self.data.extend_from_slice(&bytes);
            }
            5 => {
                let number = exact_integer(value)?;
                self.int(i32::try_from(number).context("原生 i32 值超出范围")?);
            }
            6 => {
                let number = value.as_f64().context("原生 f64 值无效")?;
                ensure!(number.is_finite(), "原生 f64 值不是有限数字");
                self.data.extend_from_slice(&number.to_le_bytes());
            }
            7 => {
                let number = value.as_f64().context("原生 f32 值无效")? as f32;
                ensure!(number.is_finite(), "原生 f32 值超出范围");
                self.data.extend_from_slice(&number.to_le_bytes());
            }
            8 => self.string(value.as_str())?,
            25 => self
                .data
                .push(u8::from(value.as_bool().context("原生布尔值无效")?)),
            29 => self
                .data
                .extend_from_slice(&exact_integer(value)?.to_le_bytes()),
            _ => bail!("不支持写入原生标记 {}", token & 31),
        }
        Ok(())
    }
}

fn is_dictionary(value: &Value) -> bool {
    value.is_object()
        && !["$ref", "$guid", "$binary"]
            .iter()
            .any(|key| value.get(key).is_some())
}

fn scalar_compatible(value: &Value, token: i32) -> bool {
    match token {
        3 | 16 | 19 | 20 | 21 | 22 => value.get("$ref").and_then(Value::as_str).is_some(),
        17 => value.get("$guid").and_then(Value::as_str).is_some(),
        4 => value.get("$binary").and_then(Value::as_str).is_some(),
        5 | 6 | 7 | 29 => value.is_number(),
        8 => value.is_null() || value.is_string(),
        25 => value.is_boolean(),
        _ => false,
    }
}

fn infer_scalar_token(value: &Value) -> Result<i32> {
    if value.get("$guid").is_some() {
        return Ok(17);
    }
    if value.get("$binary").is_some() {
        return Ok(4);
    }
    if value.get("$ref").is_some() {
        let token = value.get("$token").and_then(Value::as_i64).unwrap_or(3) as i32;
        ensure!(
            matches!(token & 31, 3 | 16 | 19 | 20 | 21 | 22),
            "原生引用标记无效"
        );
        return Ok(token);
    }
    Ok(match value {
        Value::Null | Value::String(_) => 8,
        Value::Bool(_) => 25,
        Value::Number(n) if n.is_i64() || n.is_u64() => {
            let n = exact_integer(value)?;
            if i32::try_from(n).is_ok() { 5 } else { 29 }
        }
        Value::Number(_) => 6,
        _ => bail!("无法编码原生值"),
    })
}

fn exact_integer(value: &Value) -> Result<i64> {
    if let Some(number) = value.as_i64() {
        return Ok(number);
    }
    let number = value.as_f64().context("原生整数值无效")?;
    ensure!(
        number.is_finite()
            && number.fract() == 0.0
            && number >= i64::MIN as f64
            && number < -(i64::MIN as f64),
        "原生整数值超出范围或包含小数"
    );
    Ok(number as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(out: &mut Vec<u8>, n: i32) {
        out.extend_from_slice(&n.to_le_bytes());
    }

    fn fixture() -> Vec<u8> {
        let mut out = Vec::new();
        for n in [27, 11, 0, 0, 0, 50001, 31, 3] {
            int(&mut out, n);
        }
        for text in ["Axure:Page", "name", "页面一"] {
            int(&mut out, text.len() as i32);
            out.extend_from_slice(text.as_bytes());
        }
        int(&mut out, 30);
        int(&mut out, 1);
        out.extend_from_slice(&[
            0x78, 0x56, 0x34, 0x12, 0xbc, 0x9a, 0xf0, 0xde, 1, 2, 3, 4, 5, 6, 7, 8,
        ]);
        for n in [26, 15, 1, 18, 1, 9, 1, 1, 1, 1, 8, 2, 8, 3, 28] {
            int(&mut out, n);
        }
        out
    }

    #[test]
    fn reads_real_schema_and_dotnet_guid_byte_order() {
        let graph = parse(&fixture()).unwrap();
        assert_eq!(graph.root, "123456789abcdef00102030405060708");
        assert_eq!(graph.root().unwrap().kind, "Axure:Page");
        assert_eq!(graph.root().unwrap().properties["name"], "页面一");
    }

    #[test]
    fn rejects_truncated_data_and_invalid_table_references() {
        let data = fixture();
        for len in [0, 4, 32, data.len() - 1] {
            assert!(parse(&data[..len]).is_err());
        }
        let mut invalid = data;
        let len = invalid.len();
        invalid[len - 8..len - 4].copy_from_slice(&99_i32.to_le_bytes());
        assert!(parse(&invalid).is_err());
    }

    #[test]
    fn native_write_preserves_bytes_and_appends_edited_strings() {
        let original = fixture();
        let mut graph = parse(&original).unwrap();
        assert!(encode(&graph).unwrap() == original);
        let root = graph.root.clone();
        graph.objects.get_mut(&root).unwrap().properties["name"] = "修改后的页面".into();
        let saved = encode(&graph).unwrap();
        let reopened = parse(&saved).unwrap();
        assert_eq!(reopened.root().unwrap().properties["name"], "修改后的页面");
        assert!(encode(&reopened).unwrap() == saved);
    }

    fn typed_fixture() -> Vec<u8> {
        let strings = [
            "Axure:Page",
            "f32",
            "f64",
            "long",
            "nan",
            "typed",
            "enum value",
            "zero",
            "bool",
            "binary",
            "ref",
        ];
        let mut out = Vec::new();
        for n in [27, 11, 0, 0, 0, 50001, 31, strings.len() as i32] {
            int(&mut out, n);
        }
        for text in strings {
            int(&mut out, text.len() as i32);
            out.extend_from_slice(text.as_bytes());
        }
        for n in [30, 1] {
            int(&mut out, n);
        }
        out.extend_from_slice(&[1; 16]);
        for n in [26, 15, 1, 18, 1, 9, 1, 1, 1, 9] {
            int(&mut out, n);
        }
        for n in [8, 2, 7 | 0x40] {
            int(&mut out, n);
        }
        out.extend_from_slice(&1.25_f32.to_le_bytes());
        for n in [8, 3, 6] {
            int(&mut out, n);
        }
        out.extend_from_slice(&1.25_f64.to_le_bytes());
        for n in [8, 4, 29] {
            int(&mut out, n);
        }
        out.extend_from_slice(&(i64::MAX - 11).to_le_bytes());
        for n in [8, 5, 6] {
            int(&mut out, n);
        }
        out.extend_from_slice(&0x7ff8_0000_0000_0051_u64.to_le_bytes());
        for n in [8, 6, 1, 2, 5, 42, 8, 7, 17, 1, 25] {
            int(&mut out, n);
        }
        out.push(2); // Preserve noncanonical true bits as well as typed map keys.
        for n in [8, 8, 6] {
            int(&mut out, n);
        }
        out.extend_from_slice(&(-0.0_f64).to_le_bytes());
        for n in [8, 9, 25] {
            int(&mut out, n);
        }
        out.push(2);
        for n in [8, 10, 4, 3] {
            int(&mut out, n);
        }
        out.extend_from_slice(&[0, 7, 255]);
        for n in [8, 11, 3 | 0x20, 1, 28] {
            int(&mut out, n);
        }
        out
    }

    #[test]
    fn preserves_numeric_widths_flags_nonfinite_bits_and_typed_keys_when_editing() {
        let original = typed_fixture();
        let mut graph = parse(&original).unwrap();
        assert!(encode(&graph).unwrap() == original);
        let root = graph.root.clone();
        let properties = &mut graph.objects.get_mut(&root).unwrap().properties;
        properties["f32"] = json!(3.5);
        properties["typed"]["42"] = "edited enum value".into();
        properties["new-property"] = json!({"nested":[true,"新增",1234567890123_i64]});
        let saved = encode(&graph).unwrap();
        let reopened = parse(&saved).unwrap();
        assert_eq!(reopened.root().unwrap().properties["f32"], json!(3.5));
        assert_eq!(
            reopened.root().unwrap().properties["long"],
            json!(i64::MAX - 11)
        );
        assert_eq!(
            reopened.root().unwrap().properties["typed"]["42"],
            "edited enum value"
        );
        assert!(
            saved
                .windows(8)
                .any(|w| w == 0x7ff8_0000_0000_0051_u64.to_le_bytes())
        );
        let layout = &reopened.encoding.as_ref().unwrap().records[&root].value;
        let ValueLayout::Dictionary { entries, .. } = layout else {
            panic!("dictionary");
        };
        let f32_entry = entries
            .iter()
            .find(|entry| entry.json_key == "f32")
            .unwrap();
        assert!(
            matches!(f32_entry.value_layout, ValueLayout::Scalar { token, .. } if token == (7 | 0x40))
        );
        let typed_entry = entries
            .iter()
            .find(|entry| entry.json_key == "typed")
            .unwrap();
        let ValueLayout::Dictionary { entries, .. } = &typed_entry.value_layout else {
            panic!("dictionary");
        };
        assert!(matches!(
            entries[0].key_layout,
            ValueLayout::Scalar { token: 5, .. }
        ));
        assert!(encode(&reopened).unwrap() == saved);
    }

    #[test]
    fn rejects_unrepresentable_edits_in_existing_integer_fields() {
        let mut graph = parse(&typed_fixture()).unwrap();
        let root = graph.root.clone();
        graph.objects.get_mut(&root).unwrap().properties["typed"]["42"] = "updated".into();
        graph.objects.get_mut(&root).unwrap().properties["long"] = json!(1.5);
        assert!(encode(&graph).is_err());
    }

    #[test]
    fn new_interaction_maps_write_object_reference_keys() {
        let mut graph = parse(&fixture()).unwrap();
        let id = "11111111222233334444555555555555";
        let event = "99999999222233334444555555555555";
        graph.objects.insert(
            id.into(),
            Node {
                kind: "Axure:InteractionTreeMap".into(),
                properties: json!({"node-table":{event:{"$ref":event,"$token":3}}}),
            },
        );
        graph
            .set_dictionary_key_token(id, "/node-table", 3)
            .unwrap();
        let reopened = parse(&encode(&graph).unwrap()).unwrap();
        let ValueLayout::Dictionary { entries, .. } =
            &reopened.encoding.as_ref().unwrap().records[id].value
        else {
            panic!("dictionary");
        };
        let ValueLayout::Dictionary { entries, .. } = &entries[0].value_layout else {
            panic!("dictionary");
        };
        assert!(matches!(
            entries[0].key_layout,
            ValueLayout::Scalar { token: 3, .. }
        ));
        assert_eq!(
            reopened.objects[id].properties["node-table"][event]["$ref"],
            event
        );
    }

    #[test]
    fn copied_templates_preserve_nan_widths_and_rebase_foreign_table_indices() {
        let source = parse(&typed_fixture()).unwrap();
        let mut target = parse(&fixture()).unwrap();
        let new_id = "11111111222233334444555555555555";
        target
            .copy_node_from(&source, &source.root, new_id)
            .unwrap();
        target.objects.get_mut(new_id).unwrap().properties["f32"] = json!(2.75);
        let encoded = encode(&target).unwrap();
        let reopened = parse(&encoded).unwrap();
        assert_eq!(
            reopened.objects[new_id].properties["typed"]["42"],
            "enum value"
        );
        assert_eq!(reopened.objects[new_id].properties["f32"], json!(2.75));
        assert_eq!(
            reopened.objects[new_id].properties["ref"]["$ref"],
            source.root
        );
        assert!(
            encoded
                .windows(8)
                .any(|w| w == 0x7ff8_0000_0000_0051_u64.to_le_bytes())
        );
        let ValueLayout::Dictionary { entries, .. } =
            &reopened.encoding.as_ref().unwrap().records[new_id].value
        else {
            panic!("dictionary");
        };
        let f32_entry = entries
            .iter()
            .find(|entry| entry.json_key == "f32")
            .unwrap();
        assert!(
            matches!(f32_entry.value_layout, ValueLayout::Scalar { token, .. } if token == (7 | 0x40))
        );
        assert!(encode(&reopened).unwrap() == encoded);
        // Creating a new native graph can also retain a foreign template's hints.
        let root = "aaaaaaaa222233334444555555555555";
        let mut fresh = Graph::new([11, 0, 0, 0], root.into(), root.into(), BTreeMap::new());
        fresh.copy_node_from(&source, &source.root, root).unwrap();
        let reopened = parse(&encode(&fresh).unwrap()).unwrap();
        assert_eq!(reopened.root, root);
        assert_eq!(
            reopened.root().unwrap().properties["long"],
            json!(i64::MAX - 11)
        );
    }

    #[test]
    #[ignore = "Reads local RP 11 shipped samples; run explicitly for native format validation"]
    fn every_installed_axure_package_roundtrips_byte_exactly() {
        use super::super::container;
        let root = std::env::var_os("RPSHOP_AXURE_SAMPLE_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(
                    r"D:\Program Files (x86)\Axure\Axure RP 11\DefaultSettings",
                )
            });
        assert!(
            root.is_dir(),
            "Native sample root missing: {}",
            root.display()
        );
        fn offsets(node: &Value, result: &mut Vec<u64>) {
            if let Some(parts) = node.get("parts").and_then(Value::as_object) {
                for (name, offset) in parts {
                    if matches!(name.as_str(), "package" | "DesignDocument") {
                        result.push(offset.as_u64().unwrap());
                    }
                }
            }
            if let Some(children) = node.get("children").and_then(Value::as_object) {
                for child in children.values() {
                    offsets(child, result);
                }
            }
        }
        let mut files = 0;
        let mut packages = 0;
        let mut objects = 0;
        for directory in ["Training", "Libraries"] {
            for entry in std::fs::read_dir(root.join(directory)).unwrap() {
                let path = entry.unwrap().path();
                if !matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("rp" | "rplib")
                ) {
                    continue;
                }
                let native = container::open(&path).unwrap();
                let mut parts = Vec::new();
                offsets(&native.index["root"], &mut parts);
                for offset in &parts {
                    let bytes = native.part(*offset).unwrap();
                    let graph = parse(&bytes).unwrap_or_else(|error| {
                        panic!("{} part {offset}: {error:#}", path.display())
                    });
                    objects += graph.objects.len();
                    let saved = encode(&graph).unwrap_or_else(|error| {
                        panic!("{} part {offset}: {error:#}", path.display())
                    });
                    assert!(
                        saved == bytes,
                        "{} part {offset}: native bytes changed",
                        path.display()
                    );
                    let reopened = parse(&saved).unwrap();
                    assert_eq!(graph.root, reopened.root);
                    assert_eq!(graph.objects.len(), reopened.objects.len());
                    packages += 1;
                }
                files += 1;
                eprintln!(
                    "{}: {} native packages roundtripped exactly",
                    path.display(),
                    parts.len()
                );
            }
        }
        assert!(files >= 9 && packages >= 1253 && objects >= 41374);
        eprintln!("Validated {files} files, {packages} packages, {objects} objects");
    }
}
