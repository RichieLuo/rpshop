//! Read the native binary object graph without loading or executing editor code.
//!
//! The schema is documented in `docs/native-format.md`. All lengths and table
//! references are checked before allocation, and unknown tokens are errors.

mod writer;

mod reader;
pub use reader::parse;

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

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
    strings: Vec<Option<String>>,
    guids: Vec<String>,
    values_read: usize,
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
mod tests;
