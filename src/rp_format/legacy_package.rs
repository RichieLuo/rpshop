//! Bounded reader/writer for the linked 4 KiB packages in older RP documents.
//!
//! Each physical page contains an owner ID, 4084 payload bytes, and an eight
//! byte continuation/terminal trailer. A directory in item zero names the data
//! streams. Changed streams are appended; untouched and orphaned pages remain
//! byte-for-byte intact. Directory integer fields are patched in place, which
//! also preserves fields this reader does not interpret.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};

const PAGE_BYTES: usize = 4096;
const PAYLOAD_BYTES: usize = PAGE_BYTES - 12;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ITEMS: usize = 65_536;
const MAX_VALUES: usize = 1_000_000;

#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
struct Entry {
    id: i32,
    first_page_span: usize,
    length_span: usize,
}

#[derive(Debug, Clone)]
struct Segment {
    page: usize,
    length: usize,
}

#[derive(Debug, Clone)]
pub struct Package {
    original: Vec<u8>,
    directory: Vec<u8>,
    directory_pages: Vec<Segment>,
    entries: Vec<Entry>,
    items: Vec<Item>,
}

/// Decode a package while retaining its original byte layout for native saving.
pub fn parse(bytes: &[u8]) -> Result<Package> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_BYTES && bytes.len().is_multiple_of(PAGE_BYTES),
        "旧版原生对象包长度无效"
    );
    let mut used = BTreeSet::new();
    let (directory, directory_pages) = read_chain(bytes, 0, 0, &mut used)?;
    let fields = DirectoryReader::new(&directory).read()?;
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut entries = Vec::with_capacity(fields.len());
    let mut items = Vec::with_capacity(fields.len());
    for fields in fields {
        let name = fields
            .get("name")
            .and_then(|v| v.text.as_ref())
            .context("旧版原生对象包项目缺少名称")?
            .clone();
        ensure!(names.insert(name.clone()), "旧版原生对象包项目名称重复");
        let id = fields
            .get("id")
            .and_then(|v| v.integer)
            .context("旧版原生对象包项目缺少 ID")?;
        ensure!(id > 0 && ids.insert(id), "旧版原生对象包项目 ID 无效或重复");
        let first = fields
            .get("first-page")
            .context("旧版原生对象包项目缺少首部页")?;
        let length = fields.get("Length").context("旧版原生对象包项目缺少长度")?;
        let first_page = first.integer.context("旧版原生对象包首部页不是整数")?;
        let expected_len = length.integer.context("旧版原生对象包长度不是整数")?;
        ensure!(
            first_page > 0 && expected_len >= 0,
            "旧版原生对象包项目位置无效"
        );
        let (data, _) = read_chain(bytes, first_page as usize, id, &mut used)?;
        ensure!(
            data.len() == expected_len as usize,
            "旧版原生对象包项目长度与目录不符"
        );
        entries.push(Entry {
            id,
            first_page_span: first.span,
            length_span: length.span,
        });
        items.push(Item { name, data });
    }
    Ok(Package {
        original: bytes.to_vec(),
        directory,
        directory_pages,
        entries,
        items,
    })
}

impl Package {
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Replace existing named streams while preserving every untouched page.
    /// An empty replacement set or identical values returns the exact input.
    pub fn encode_with_replacements(
        &self,
        replacements: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Vec<u8>> {
        ensure!(
            replacements
                .keys()
                .all(|name| self.items.iter().any(|item| item.name == *name)),
            "旧版原生对象包替换项目不存在"
        );
        let mut bytes = self.original.clone();
        let mut directory = self.directory.clone();
        let mut changed = false;
        for (item, entry) in self.items.iter().zip(&self.entries) {
            let Some(data) = replacements.get(&item.name) else {
                continue;
            };
            if data == &item.data {
                continue;
            }
            ensure!(
                data.len() <= MAX_BYTES,
                "旧版原生对象包项目超过 64 MiB 限制"
            );
            let first_page = append_chain(&mut bytes, entry.id, data)?;
            patch_integer(&mut directory, entry.first_page_span, first_page)?;
            patch_integer(&mut directory, entry.length_span, data.len())?;
            changed = true;
        }
        if changed {
            let mut cursor = 0;
            for segment in &self.directory_pages {
                let start = segment.page * PAGE_BYTES + 4;
                bytes[start..start + segment.length]
                    .copy_from_slice(&directory[cursor..cursor + segment.length]);
                cursor += segment.length;
            }
            ensure!(cursor == directory.len(), "旧版原生对象包目录写回长度无效");
        }
        Ok(bytes)
    }
}

fn integer(bytes: &[u8], offset: usize) -> Result<i32> {
    let end = offset.checked_add(4).context("旧版原生对象包偏移溢出")?;
    let value = bytes.get(offset..end).context("旧版原生对象包数据不完整")?;
    Ok(i32::from_le_bytes(value.try_into()?))
}

fn patch_integer(bytes: &mut [u8], offset: usize, value: usize) -> Result<()> {
    let value = i32::try_from(value).context("旧版原生对象包整数超过范围")?;
    let end = offset.checked_add(4).context("旧版原生对象包偏移溢出")?;
    bytes
        .get_mut(offset..end)
        .context("旧版原生对象包目录位置无效")?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn read_chain(
    bytes: &[u8],
    mut page: usize,
    owner: i32,
    used: &mut BTreeSet<usize>,
) -> Result<(Vec<u8>, Vec<Segment>)> {
    let mut data = Vec::new();
    let mut segments = Vec::new();
    loop {
        ensure!(
            page < bytes.len() / PAGE_BYTES && used.insert(page),
            "旧版原生对象包页链越界、循环或重叠"
        );
        let start = page * PAGE_BYTES;
        ensure!(
            integer(bytes, start)? == owner,
            "旧版原生对象包页归属与目录不符"
        );
        let kind = integer(bytes, start + PAGE_BYTES - 8)?;
        let value = integer(bytes, start + PAGE_BYTES - 4)?;
        let length = match kind {
            1 => PAYLOAD_BYTES,
            2 => {
                ensure!(
                    (0..=PAYLOAD_BYTES as i32).contains(&value),
                    "旧版原生对象包末页长度无效"
                );
                value as usize
            }
            _ => bail!("旧版原生对象包页链标记无效：{kind}"),
        };
        ensure!(
            data.len() + length <= MAX_BYTES,
            "旧版原生对象包项目超过 64 MiB 限制"
        );
        data.extend_from_slice(&bytes[start + 4..start + 4 + length]);
        segments.push(Segment { page, length });
        if kind == 2 {
            return Ok((data, segments));
        }
        ensure!(value > 0, "旧版原生对象包续页位置无效");
        page = value as usize;
    }
}

fn append_chain(bytes: &mut Vec<u8>, owner: i32, data: &[u8]) -> Result<usize> {
    let first = bytes.len() / PAGE_BYTES;
    let pages = data.len().max(1).div_ceil(PAYLOAD_BYTES);
    let total = pages
        .checked_mul(PAGE_BYTES)
        .and_then(|n| bytes.len().checked_add(n))
        .context("旧版原生对象包长度溢出")?;
    ensure!(total <= MAX_BYTES, "旧版原生对象包超过 64 MiB 写入限制");
    bytes.resize(total, 0);
    for index in 0..pages {
        let start = (first + index) * PAGE_BYTES;
        bytes[start..start + 4].copy_from_slice(&owner.to_le_bytes());
        let offset = index * PAYLOAD_BYTES;
        let length = data.len().saturating_sub(offset).min(PAYLOAD_BYTES);
        bytes[start + 4..start + 4 + length].copy_from_slice(&data[offset..offset + length]);
        let last = index + 1 == pages;
        let kind = if last { 2i32 } else { 1i32 };
        bytes[start + PAGE_BYTES - 8..start + PAGE_BYTES - 4].copy_from_slice(&kind.to_le_bytes());
        patch_integer(
            bytes,
            start + PAGE_BYTES - 4,
            if last { length } else { first + index + 1 },
        )?;
    }
    Ok(first)
}

#[derive(Default)]
struct Field {
    text: Option<String>,
    integer: Option<i32>,
    span: usize,
    list: Option<Vec<Field>>,
    dictionary: Option<BTreeMap<String, Field>>,
}

struct DirectoryReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
    strings: Vec<Option<String>>,
    values: usize,
}

impl<'a> DirectoryReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            cursor: 0,
            strings: vec![None],
            values: 0,
        }
    }

    fn i32(&mut self) -> Result<i32> {
        let value = integer(self.bytes, self.cursor)?;
        self.cursor += 4;
        Ok(value)
    }

    fn expect(&mut self, expected: i32) -> Result<()> {
        let value = self.i32()?;
        ensure!(value == expected, "旧版原生对象包目录标记无效：{value}");
        Ok(())
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .cursor
            .checked_add(length)
            .context("旧版原生对象包目录长度溢出")?;
        let data = self
            .bytes
            .get(self.cursor..end)
            .context("旧版原生对象包目录不完整")?;
        self.cursor = end;
        Ok(data)
    }

    fn count(&mut self) -> Result<usize> {
        let count = self.i32()?;
        ensure!(
            (0..=MAX_VALUES as i32).contains(&count),
            "旧版原生对象包目录数量无效"
        );
        Ok(count as usize)
    }

    fn read(mut self) -> Result<Vec<BTreeMap<String, Field>>> {
        self.expect(27)?;
        for _ in 0..4 {
            self.i32()?;
        }
        self.expect(50001)?;
        self.expect(31)?;
        let count = self.count()?;
        ensure!(
            count <= self.bytes.len() / 4,
            "旧版原生对象包目录字符串数量无效"
        );
        for _ in 0..count {
            let length = self.count()?;
            let value = std::str::from_utf8(self.take(length)?)
                .context("旧版原生对象包目录字符串不是 UTF-8")?;
            self.strings.push(Some(value.to_owned()));
        }
        self.expect(30)?;
        let guids = self.count()?;
        self.take(
            guids
                .checked_mul(16)
                .context("旧版原生对象包目录 GUID 数量溢出")?,
        )?;
        let mut root = self
            .value(0)?
            .dictionary
            .context("旧版原生对象包目录根不是字典")?;
        self.expect(28)?;
        ensure!(
            self.cursor == self.bytes.len(),
            "旧版原生对象包目录后存在未知数据"
        );
        let items = root
            .remove("item-infos")
            .and_then(|f| f.list)
            .context("旧版原生对象包目录缺少项目列表")?;
        ensure!(items.len() <= MAX_ITEMS, "旧版原生对象包项目过多");
        items
            .into_iter()
            .map(|f| f.dictionary.context("旧版原生对象包目录项目不是字典"))
            .collect()
    }

    fn value(&mut self, depth: usize) -> Result<Field> {
        ensure!(depth < 32, "旧版原生对象包目录嵌套过深");
        self.values += 1;
        ensure!(self.values <= MAX_VALUES, "旧版原生对象包目录值过多");
        let token = self.i32()? & 31;
        let mut field = Field {
            span: self.cursor,
            ..Field::default()
        };
        match token {
            1 => {
                let count = self.count()?;
                ensure!(
                    count <= (self.bytes.len() - self.cursor) / 8,
                    "旧版原生对象包目录字典长度无效"
                );
                let mut values = BTreeMap::new();
                for _ in 0..count {
                    let name = self
                        .value(depth + 1)?
                        .text
                        .context("旧版原生对象包目录字典键不是字符串")?;
                    ensure!(
                        values.insert(name, self.value(depth + 1)?).is_none(),
                        "旧版原生对象包目录字典键重复"
                    );
                }
                field.dictionary = Some(values);
            }
            2 => {
                let count = self.count()?;
                ensure!(
                    count <= (self.bytes.len() - self.cursor) / 4,
                    "旧版原生对象包目录列表长度无效"
                );
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(self.value(depth + 1)?);
                }
                field.list = Some(values);
            }
            5 => field.integer = Some(self.i32()?),
            8 => {
                let id = self.i32()?;
                ensure!(id >= 0, "旧版原生对象包目录字符串索引无效");
                field.text = self
                    .strings
                    .get(id as usize)
                    .context("旧版原生对象包目录字符串索引越界")?
                    .clone();
            }
            4 => {
                let length = self.count()?;
                self.take(length)?;
            }
            3 | 7 | 17 | 20 | 21 | 22 => {
                self.take(4)?;
            }
            6 | 16 | 19 | 29 => {
                self.take(8)?;
            }
            25 => {
                self.take(1)?;
            }
            _ => bail!("旧版原生对象包目录值标记不受支持：{token}"),
        }
        Ok(field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i32_bytes(bytes: &mut Vec<u8>, value: i32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn fixture(data: &[u8]) -> Vec<u8> {
        let mut directory = Vec::new();
        for value in [27, 0, 0, 3, 0, 50001, 31, 6] {
            i32_bytes(&mut directory, value);
        }
        for text in [
            "item-infos",
            "name",
            "package",
            "id",
            "first-page",
            "Length",
        ] {
            i32_bytes(&mut directory, text.len() as i32);
            directory.extend_from_slice(text.as_bytes());
        }
        for value in [
            30,
            0,
            1,
            1,
            8,
            1,
            2,
            1,
            1,
            4,
            8,
            2,
            8,
            3,
            8,
            4,
            5,
            1,
            8,
            5,
            5,
            1,
            8,
            6,
            5,
            data.len() as i32,
            28,
        ] {
            i32_bytes(&mut directory, value);
        }
        let mut result = Vec::new();
        append_chain(&mut result, 0, &directory).unwrap();
        append_chain(&mut result, 1, data).unwrap();
        result
    }

    #[test]
    fn linked_streams_round_trip_and_preserve_original_pages() {
        let original_data = vec![0xab; 12_000];
        let original = fixture(&original_data);
        let package = parse(&original).unwrap();
        assert_eq!(package.items()[0].data, original_data);
        assert_eq!(
            package.encode_with_replacements(&BTreeMap::new()).unwrap(),
            original
        );
        assert_eq!(
            package
                .encode_with_replacements(&BTreeMap::from([("package".into(), original_data)]))
                .unwrap(),
            original
        );
        let replacement = vec![0x37; 20_000];
        let edited = package
            .encode_with_replacements(&BTreeMap::from([("package".into(), replacement.clone())]))
            .unwrap();
        assert_eq!(&edited[PAGE_BYTES..original.len()], &original[PAGE_BYTES..]);
        assert_eq!(parse(&edited).unwrap().items()[0].data, replacement);
    }

    #[test]
    fn rejects_cycle_owner_mismatch_and_truncation() {
        let original = fixture(&vec![0; PAYLOAD_BYTES + 1]);
        let mut cycle = original.clone();
        patch_integer(&mut cycle, 2 * PAGE_BYTES - 4, 1).unwrap();
        assert!(parse(&cycle).is_err());
        let mut owner = original.clone();
        patch_integer(&mut owner, PAGE_BYTES, 9).unwrap();
        assert!(parse(&owner).is_err());
        assert!(parse(&original[..original.len() - 1]).is_err());
    }

    #[test]
    fn empty_replacement_is_a_valid_stream() {
        let package = parse(&fixture(b"original")).unwrap();
        let bytes = package
            .encode_with_replacements(&BTreeMap::from([("package".into(), Vec::new())]))
            .unwrap();
        assert!(parse(&bytes).unwrap().items()[0].data.is_empty());
    }
}
