//! ZIP-backed native documents retain their original ZIP and linked packages.
//!
//! The public index uses stable virtual offsets for the existing part-reader
//! API. Saving patches the original package hierarchy and never converts the
//! document into a newer container format.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

use super::legacy_package::{self, Package};

const MAX_FILE_BYTES: usize = 256 * 1024 * 1024;
const MAX_PART_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 16_384;
const MAX_PARTS: usize = 300_000;

#[derive(Clone)]
pub struct LegacyContainer {
    pub version: u16,
    pub index: Value,
    original: Vec<u8>,
    packages: Vec<ArchivePackage>,
    parts: BTreeMap<u64, PartLocation>,
}

#[derive(Clone)]
struct ArchivePackage {
    zip_index: usize,
    node: PackageNode,
}

#[derive(Clone)]
struct PackageNode {
    package: Package,
    nested: BTreeMap<usize, PackageNode>,
}

#[derive(Clone)]
struct PartLocation {
    package: usize,
    path: Vec<usize>,
}

#[derive(Default)]
struct BoundedOutput(Cursor<Vec<u8>>);

impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .0
            .position()
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > MAX_FILE_BYTES as u64)
        {
            return Err(io::Error::other("原生文件超过 256 MiB 写入上限"));
        }
        self.0.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Seek for BoundedOutput {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let offset = self.0.seek(position)?;
        if offset > MAX_FILE_BYTES as u64 {
            return Err(io::Error::other("原生文件超过 256 MiB 写入上限"));
        }
        Ok(offset)
    }
}

pub fn from_bytes(bytes: &[u8]) -> Result<LegacyContainer> {
    ensure!(
        bytes.len() <= MAX_FILE_BYTES,
        "原生文件超过 256 MiB 读取上限"
    );
    let mut archive = ZipArchive::new(Cursor::new(bytes)).context("旧版原生文件目录无效")?;
    ensure!(archive.len() <= MAX_ENTRIES, "旧版原生文件目录项目过多");
    let mut total = 0u64;
    let mut names = BTreeSet::new();
    let mut version = None;
    let mut design = None;
    let mut candidates = Vec::new();
    for position in 0..archive.len() {
        let entry = archive
            .by_index_raw(position)
            .context("读取旧版原生文件目录失败")?;
        let name = std::str::from_utf8(entry.name_raw()).context("旧版原生文件目录名称编码无效")?;
        validate_name(name)?;
        ensure!(
            names.insert(name.to_ascii_lowercase()),
            "旧版原生文件目录项目重复"
        );
        ensure!(!entry.encrypted(), "暂不支持加密的原生文件");
        ensure!(
            entry.size() <= MAX_PART_BYTES as u64,
            "旧版原生对象包超过 64 MiB 读取上限"
        );
        total = total
            .checked_add(entry.size())
            .context("旧版原生文件解压长度溢出")?;
        ensure!(
            total <= MAX_FILE_BYTES as u64,
            "旧版原生文件解压后超过 256 MiB 读取上限"
        );
        if let Some(text) = name.strip_suffix(".version")
            && !text.contains('/')
        {
            let mut numbers = text.split('.');
            let major = numbers.next().and_then(|value| value.parse::<u16>().ok());
            ensure!(
                major.is_some()
                    && numbers.all(|value| !value.is_empty()
                        && value.bytes().all(|byte| byte.is_ascii_digit())),
                "旧版原生文件版本标记无效"
            );
            ensure!(
                version.replace(major.unwrap()).is_none(),
                "旧版原生文件版本标记重复"
            );
        }
        if entry.is_dir() || !name.ends_with(".rppkg") {
            continue;
        }
        let kind = name.split('/').next().unwrap_or("");
        if matches!(kind, "DesignDocument" | "Page" | "Master") {
            if kind == "DesignDocument" {
                ensure!(
                    design.replace(position).is_none(),
                    "旧版原生文件包含重复文档信息"
                );
            }
            candidates.push((position, kind == "DesignDocument", name.to_owned()));
        }
    }
    let version = version.context("不是受支持的旧版 RP 原生文件：缺少版本标记")?;
    ensure!(
        version == 8,
        "暂不支持旧版 RP 文件版本 {version}，当前支持 RP 8 和 RP 11"
    );
    ensure!(design.is_some(), "旧版原生文件缺少文档信息");

    let mut packages = Vec::new();
    let mut parts = BTreeMap::new();
    let mut root_parts = Map::new();
    let mut root_children = Map::new();
    let mut decoded_total = 0usize;
    for (zip_index, is_design, name) in candidates {
        let mut entry = archive
            .by_index(zip_index)
            .context("读取旧版原生对象包失败")?;
        let mut data = Vec::new();
        entry
            .by_ref()
            .take(MAX_PART_BYTES as u64 + 1)
            .read_to_end(&mut data)
            .with_context(|| format!("读取旧版原生对象包 {name} 失败"))?;
        ensure!(
            data.len() <= MAX_PART_BYTES,
            "旧版原生对象包超过 64 MiB 读取上限"
        );
        let package = packages.len();
        let (node, mut index) = parse_node(
            &data,
            package,
            &mut Vec::new(),
            &mut parts,
            &mut decoded_total,
            0,
        )
        .with_context(|| format!("读取旧版原生对象包 {name} 失败"))?;
        if is_design {
            let object = index["parts"].as_object_mut().unwrap();
            let offset = object
                .remove("package")
                .context("旧版原生文件缺少文档数据")?;
            ensure!(
                root_parts.insert("DesignDocument".into(), offset).is_none(),
                "旧版原生文档数据重复"
            );
            for (key, value) in std::mem::take(object) {
                ensure!(
                    root_parts.insert(key, value).is_none(),
                    "旧版原生文档部件重复"
                );
            }
        } else if !index["parts"].as_object().unwrap().is_empty() {
            // Some native libraries store their primary package without an
            // outer GUID wrapper. Keep the same synthetic directory shape.
            let id = name.rsplit('/').next().unwrap().trim_end_matches(".rppkg");
            ensure!(
                root_children.insert(id.into(), index.clone()).is_none(),
                "旧版原生对象包 ID 重复"
            );
            index["children"] = json!({});
        }
        for (key, value) in std::mem::take(index["children"].as_object_mut().unwrap()) {
            ensure!(
                root_children.insert(key, value).is_none(),
                "旧版原生对象包 ID 重复"
            );
        }
        packages.push(ArchivePackage { zip_index, node });
    }
    Ok(LegacyContainer {
        version,
        index: json!({"root":{"parts":root_parts,"children":root_children}}),
        original: bytes.to_vec(),
        packages,
        parts,
    })
}

fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 4096
            && !name.starts_with('/')
            && !name.contains(['\\', ':', '\0']),
        "旧版原生文件目录名称无效"
    );
    ensure!(
        name.split('/').all(|part| part != "." && part != ".."),
        "旧版原生文件目录包含不安全路径"
    );
    Ok(())
}

fn parse_node(
    bytes: &[u8],
    package_index: usize,
    path: &mut Vec<usize>,
    parts: &mut BTreeMap<u64, PartLocation>,
    total: &mut usize,
    depth: usize,
) -> Result<(PackageNode, Value)> {
    ensure!(depth <= 32, "旧版原生对象包嵌套过深");
    *total = total
        .checked_add(bytes.len())
        .context("旧版原生对象包总长度溢出")?;
    ensure!(
        *total <= MAX_FILE_BYTES,
        "旧版原生对象包展开后超过 256 MiB 上限"
    );
    let package = legacy_package::parse(bytes)?;
    let mut nested = BTreeMap::new();
    let mut index_parts = Map::new();
    let mut children = Map::new();
    for (position, item) in package.items().iter().enumerate() {
        path.push(position);
        // Only a linked package has a zero-owner page followed by the native
        // binary directory marker. Binary graphs/images are left untouched.
        if item.data.len().is_multiple_of(4096) && item.data.starts_with(&[0, 0, 0, 0, 27, 0, 0, 0])
        {
            let (node, index) =
                parse_node(&item.data, package_index, path, parts, total, depth + 1)?;
            nested.insert(position, node);
            children.insert(item.name.clone(), index);
        } else {
            ensure!(parts.len() < MAX_PARTS, "旧版原生文件部件数量超过上限");
            let offset = parts.len() as u64;
            parts.insert(
                offset,
                PartLocation {
                    package: package_index,
                    path: path.clone(),
                },
            );
            index_parts.insert(item.name.clone(), Value::from(offset));
        }
        path.pop();
    }
    Ok((
        PackageNode { package, nested },
        json!({"parts":index_parts,"children":children}),
    ))
}

impl LegacyContainer {
    pub fn original_bytes(&self) -> &[u8] {
        &self.original
    }

    pub fn part(&self, offset: u64) -> Result<Vec<u8>> {
        let location = self.parts.get(&offset).context("旧版原生部件不存在")?;
        let mut node = &self.packages[location.package].node;
        let (leaf, parents) = location.path.split_last().context("旧版原生部件路径为空")?;
        for position in parents {
            node = node.nested.get(position).context("旧版原生部件路径无效")?;
        }
        Ok(node.package.items()[*leaf].data.clone())
    }

    pub fn encode_with_parts(
        &self,
        index: &Value,
        replacements: &BTreeMap<u64, Vec<u8>>,
    ) -> Result<Vec<u8>> {
        ensure!(
            index == &self.index,
            "旧版原生文件暂不支持增删对象包；已有元件仍可编辑保存"
        );
        let mut changed: BTreeMap<usize, BTreeMap<Vec<usize>, Vec<u8>>> = BTreeMap::new();
        for (offset, bytes) in replacements {
            ensure!(
                bytes.len() <= MAX_PART_BYTES,
                "旧版原生部件超过 64 MiB 写入上限"
            );
            let location = self.parts.get(offset).context("替换的旧版原生部件不存在")?;
            if self.part(*offset)? != *bytes {
                changed
                    .entry(location.package)
                    .or_default()
                    .insert(location.path.clone(), bytes.clone());
            }
        }
        if changed.is_empty() {
            return Ok(self.original.clone());
        }
        let mut rewritten = BTreeMap::new();
        for (position, replacements) in changed {
            let package = &self.packages[position];
            rewritten.insert(package.zip_index, package.node.encode(&replacements)?);
        }
        let mut source =
            ZipArchive::new(Cursor::new(&self.original)).context("旧版原生文件目录无效")?;
        let mut writer = ZipWriter::new(BoundedOutput::default());
        writer.set_raw_comment(source.comment().into());
        let mut expanded = 0u64;
        for position in 0..source.len() {
            let entry = source
                .by_index_raw(position)
                .context("读取旧版原生文件目录失败")?;
            expanded = expanded
                .checked_add(
                    rewritten
                        .get(&position)
                        .map_or(entry.size(), |data| data.len() as u64),
                )
                .context("旧版原生文件解压长度溢出")?;
            ensure!(
                expanded <= MAX_FILE_BYTES as u64,
                "旧版原生文件解压后超过 256 MiB 写入上限"
            );
            if let Some(bytes) = rewritten.get(&position) {
                let mut options =
                    SimpleFileOptions::default().compression_method(entry.compression());
                if let Some(time) = entry.last_modified() {
                    options = options.last_modified_time(time);
                }
                if let Some(mode) = entry.unix_mode() {
                    options = options.unix_permissions(mode);
                }
                writer
                    .start_file(entry.name(), options)
                    .context("写入旧版原生对象包目录失败")?;
                writer.write_all(bytes).context("写入旧版原生对象包失败")?;
            } else {
                writer
                    .raw_copy_file(entry)
                    .context("保留旧版原生文件目录项目失败")?;
            }
        }
        let bytes = writer
            .finish()
            .context("完成旧版原生文件目录失败")?
            .0
            .into_inner();
        ensure!(
            bytes.len() <= MAX_FILE_BYTES,
            "原生文件超过 256 MiB 写入上限"
        );
        Ok(bytes)
    }
}

impl PackageNode {
    fn encode(&self, replacements: &BTreeMap<Vec<usize>, Vec<u8>>) -> Result<Vec<u8>> {
        let mut direct = BTreeMap::new();
        let mut nested: BTreeMap<usize, BTreeMap<Vec<usize>, Vec<u8>>> = BTreeMap::new();
        for (path, bytes) in replacements {
            let (first, rest) = path.split_first().context("旧版原生替换部件路径为空")?;
            if rest.is_empty() {
                direct.insert(self.package.items()[*first].name.clone(), bytes.clone());
            } else {
                nested
                    .entry(*first)
                    .or_default()
                    .insert(rest.to_vec(), bytes.clone());
            }
        }
        for (position, replacements) in nested {
            let node = self
                .nested
                .get(&position)
                .context("旧版原生替换部件路径无效")?;
            direct.insert(
                self.package.items()[position].name.clone(),
                node.encode(&replacements)?,
            );
        }
        self.package.encode_with_replacements(&direct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rp_format::container;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };
    use zip::CompressionMethod;

    fn integer(bytes: &mut Vec<u8>, value: i32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn chain(bytes: &mut Vec<u8>, owner: i32, data: &[u8]) -> usize {
        let first = bytes.len() / 4096;
        let count = data.len().max(1).div_ceil(4084);
        for index in 0..count {
            let start = bytes.len();
            bytes.resize(start + 4096, 0);
            bytes[start..start + 4].copy_from_slice(&owner.to_le_bytes());
            let offset = index * 4084;
            let length = data.len().saturating_sub(offset).min(4084);
            bytes[start + 4..start + 4 + length].copy_from_slice(&data[offset..offset + length]);
            let last = index + 1 == count;
            bytes[start + 4088..start + 4092]
                .copy_from_slice(&(if last { 2i32 } else { 1i32 }).to_le_bytes());
            bytes[start + 4092..start + 4096].copy_from_slice(
                &(if last { length } else { first + index + 1 } as i32).to_le_bytes(),
            );
        }
        first
    }

    fn package(items: &[(&str, &[u8])]) -> Vec<u8> {
        let mut bytes = vec![0; 4096];
        let positions = items
            .iter()
            .enumerate()
            .map(|(index, (_, data))| chain(&mut bytes, index as i32 + 1, data))
            .collect::<Vec<_>>();
        let mut directory = Vec::new();
        for value in [27, 0, 0, 3, 0, 50001, 31, 5 + items.len() as i32] {
            integer(&mut directory, value);
        }
        for name in ["item-infos", "name", "id", "first-page", "Length"]
            .into_iter()
            .chain(items.iter().map(|(name, _)| *name))
        {
            integer(&mut directory, name.len() as i32);
            directory.extend_from_slice(name.as_bytes());
        }
        for value in [30, 0, 1, 1, 8, 1, 2, items.len() as i32] {
            integer(&mut directory, value);
        }
        for (position, (_, data)) in items.iter().enumerate() {
            for value in [
                1,
                4,
                8,
                2,
                8,
                position as i32 + 6,
                8,
                3,
                5,
                position as i32 + 1,
                8,
                4,
                5,
                positions[position] as i32,
                8,
                5,
                5,
                data.len() as i32,
            ] {
                integer(&mut directory, value);
            }
        }
        integer(&mut directory, 28);
        assert!(directory.len() <= 4084);
        let mut header = Vec::new();
        chain(&mut header, 0, &directory);
        bytes[..4096].copy_from_slice(&header);
        bytes
    }

    fn archive(extra_name: &str) -> Vec<u8> {
        let design = package(&[("package", b"document graph")]);
        let page = package(&[
            ("package", b"page graph"),
            ("thumbnail", b"original thumbnail"),
        ]);
        let state = package(&[("package", b"state graph")]);
        let outer = package(&[("primary-page", &page), ("panel-state", &state)]);
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.set_comment("preserve archive comment");
        for (name, data) in [
            ("8.0.0.3303.version", b"<version>8</version>".as_slice()),
            (
                "DesignDocument/ab/document/document.rppkg",
                design.as_slice(),
            ),
            ("Page/cd/primary-page/primary-page.rppkg", outer.as_slice()),
            (extra_name, b"unknown future settings \0\xff".as_slice()),
        ] {
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .unix_permissions(0o640);
            writer.start_file(name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn compressed_entry(bytes: &[u8], name: &str) -> Vec<u8> {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let entry = archive.by_name(name).unwrap();
        let start = entry.data_start() as usize;
        bytes[start..start + entry.compressed_size() as usize].to_vec()
    }

    fn temporary_directory() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("rpshop-legacy-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn nested_native_edits_preserve_zip_format_and_untouched_entries() {
        let bytes = archive("future/settings.bin");
        let source = container::from_bytes(&bytes).unwrap();
        assert_eq!(source.version, 8);
        assert_eq!(source.original_bytes(), bytes);
        assert_eq!(source.encode_with_parts(&BTreeMap::new()).unwrap(), bytes);
        let page = source.index["root"]["children"]["primary-page"]["parts"]["package"]
            .as_u64()
            .unwrap();
        let state = source.index["root"]["children"]["panel-state"]["parts"]["package"]
            .as_u64()
            .unwrap();
        let thumbnail = source.index["root"]["children"]["primary-page"]["parts"]["thumbnail"]
            .as_u64()
            .unwrap();
        assert_eq!(source.part(page).unwrap(), b"page graph");
        assert_eq!(source.part(state).unwrap(), b"state graph");
        assert_eq!(
            source
                .encode_with_parts(&BTreeMap::from([(page, source.part(page).unwrap())]))
                .unwrap(),
            bytes
        );
        let replacement = vec![0x91; 20_000];
        let edited = source
            .encode_with_parts(&BTreeMap::from([(page, replacement.clone())]))
            .unwrap();
        assert!(edited.starts_with(b"PK\x03\x04"));
        let reopened = container::from_bytes(&edited).unwrap();
        assert_eq!(reopened.version, 8);
        assert_eq!(reopened.index, source.index);
        assert_eq!(reopened.part(page).unwrap(), replacement);
        assert_eq!(reopened.part(state).unwrap(), b"state graph");
        assert_eq!(reopened.part(thumbnail).unwrap(), b"original thumbnail");
        for name in [
            "8.0.0.3303.version",
            "DesignDocument/ab/document/document.rppkg",
            "future/settings.bin",
        ] {
            assert_eq!(
                compressed_entry(&edited, name),
                compressed_entry(&bytes, name)
            );
        }
        let old = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let new = ZipArchive::new(Cursor::new(&edited)).unwrap();
        assert_eq!(new.comment(), old.comment());
        assert_eq!(new.len(), old.len());
        assert_ne!(
            reopened.original_fingerprint(),
            source.original_fingerprint()
        );
    }

    #[test]
    fn rejects_unsafe_archive_paths_unknown_versions_and_unknown_parts() {
        for name in [
            "../escape",
            "/absolute",
            "C:/absolute",
            "future\\settings",
            "future/./settings",
        ] {
            assert!(
                from_bytes(&archive(name)).is_err(),
                "accepted unsafe name: {name}"
            );
        }
        let bytes = archive("future/settings.bin");
        let source = from_bytes(&bytes).unwrap();
        assert!(
            source
                .encode_with_parts(&source.index, &BTreeMap::from([(u64::MAX, Vec::new())]))
                .is_err()
        );
        let mut edited_index = source.index.clone();
        edited_index["root"]["parts"]["added"] = json!(u64::MAX);
        assert!(
            source
                .encode_with_parts(&edited_index, &BTreeMap::new())
                .is_err()
        );
        let mut unknown_version = bytes;
        for start in 0..unknown_version.len().saturating_sub(18) {
            if unknown_version[start..].starts_with(b"8.0.0.3303.version") {
                unknown_version[start] = b'9';
            }
        }
        assert!(from_bytes(&unknown_version).is_err());
    }

    #[test]
    fn checked_legacy_saving_detects_source_changes_and_keeps_original_format() {
        let directory = temporary_directory();
        let path = directory.join("native.rp");
        let bytes = archive("future/settings.bin");
        container::write_atomic(&path, &bytes).unwrap();
        let source = container::open(&path).unwrap();
        assert!(source.matches_original(&path).unwrap());
        let offset = source.index["root"]["parts"]["DesignDocument"]
            .as_u64()
            .unwrap();
        let edited = source
            .encode_with_parts(&BTreeMap::from([(
                offset,
                b"edited document graph".to_vec(),
            )]))
            .unwrap();
        let mut external = bytes.clone();
        *external.last_mut().unwrap() ^= 1;
        fs::write(&path, &external).unwrap();
        assert!(!source.matches_original(&path).unwrap());
        assert!(source.write_atomic_checked(&path, &edited).is_err());
        assert_eq!(fs::read(&path).unwrap(), external);
        fs::write(&path, &bytes).unwrap();
        source.write_atomic_checked(&path, &edited).unwrap();
        let reopened = container::open(&path).unwrap();
        assert_eq!(reopened.version, 8);
        assert_eq!(reopened.part(offset).unwrap(), b"edited document graph");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
