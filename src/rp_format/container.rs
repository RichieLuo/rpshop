//! Bounded reader for native project/library containers.
//!
//! The container layout was checked against the RP 11 bundled training projects
//! and widget libraries. It is independent of the serialization inside a part.

use anyhow::{Context, Result, bail, ensure};
use flate2::{Compression, read::MultiGzDecoder, write::GzEncoder};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_FILE_BYTES: usize = 256 * 1024 * 1024;
const MAX_PART_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct Container {
    pub version: u16,
    pub index: Value,
    payload: Vec<u8>,
    original_header: [u8; 8],
    original_index_frame: Vec<u8>,
    original_index: Value,
    legacy: Option<super::legacy::LegacyContainer>,
}

pub fn open(path: &Path) -> Result<Container> {
    let file = File::open(path).with_context(|| format!("无法打开 RP 文件：{}", path.display()))?;
    ensure!(
        file.metadata()?.len() <= MAX_FILE_BYTES as u64,
        "RP 文件超过 256 MiB 读取上限"
    );
    // The bounded read also handles a file that grows after the metadata check.
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .context("读取 RP 文件失败")?;
    from_bytes(&bytes)
}

pub fn from_bytes(bytes: &[u8]) -> Result<Container> {
    ensure!(
        bytes.len() <= MAX_FILE_BYTES,
        "RP 文件超过 256 MiB 读取上限"
    );
    if bytes.starts_with(b"PK\x03\x04") {
        let legacy = super::legacy::from_bytes(bytes)?;
        return Ok(Container {
            version: legacy.version,
            index: legacy.index.clone(),
            original_index: legacy.index.clone(),
            payload: Vec::new(),
            original_header: [0; 8],
            original_index_frame: Vec::new(),
            legacy: Some(legacy),
        });
    }
    ensure!(bytes.len() >= 8, "RP 文件头不完整");
    ensure!(bytes[..2] == [0xac, 0xef], "不是受支持的 RP 原生文件");
    let version = u16::from_le_bytes([bytes[2], bytes[3]]);
    ensure!(
        version == 11,
        "暂不支持 RP 文件版本 {version}，当前支持 RP 8 和 RP 11"
    );
    let index_len = u32::from_le_bytes(bytes[4..8].try_into()?) as usize;
    ensure!(index_len <= MAX_PART_BYTES, "原生索引超过 64 MiB 读取上限");
    let payload_start = 8usize.checked_add(index_len).context("原生索引长度溢出")?;
    let frame = bytes.get(8..payload_start).context("原生索引数据不完整")?;
    let decoded = decode_index(frame)?;
    let index: Value = serde_json::from_slice(&decoded).context("原生索引不是有效 JSON")?;
    ensure!(
        index.get("root").is_some_and(Value::is_object),
        "原生索引缺少 root 对象"
    );
    Ok(Container {
        version,
        original_index: index.clone(),
        index,
        payload: bytes[payload_start..].to_vec(),
        original_header: bytes[..8].try_into()?,
        original_index_frame: frame.to_vec(),
        legacy: None,
    })
}

impl Container {
    /// Read a part at an offset relative to the end of the index frame.
    pub fn part(&self, offset: u64) -> Result<Vec<u8>> {
        if let Some(legacy) = &self.legacy {
            return legacy.part(offset);
        }
        let start = usize::try_from(offset).context("原生部件偏移超出范围")?;
        let header_end = start.checked_add(8).context("原生部件偏移溢出")?;
        let header = self
            .payload
            .get(start..header_end)
            .context("原生部件头不完整")?;
        let tag = u32::from_le_bytes(header[..4].try_into()?);
        ensure!(tag == 0, "暂不支持 原生部件类型 {tag}");
        let length = u32::from_le_bytes(header[4..8].try_into()?) as usize;
        ensure!(length <= MAX_PART_BYTES, "原生部件超过 64 MiB 读取上限");
        let end = header_end.checked_add(length).context("原生部件长度溢出")?;
        let data = self
            .payload
            .get(header_end..end)
            .context("原生部件数据不完整")?;
        decode_part(data, MAX_PART_BYTES)
    }

    /// Return the exact bytes read from disk, including the original LZ4 frame.
    pub fn original_bytes(&self) -> Vec<u8> {
        if let Some(legacy) = &self.legacy {
            return legacy.original_bytes().to_vec();
        }
        let mut bytes = Vec::with_capacity(self.original_len());
        bytes.extend_from_slice(&self.original_header);
        bytes.extend_from_slice(&self.original_index_frame);
        bytes.extend_from_slice(&self.payload);
        bytes
    }

    fn original_len(&self) -> usize {
        if let Some(legacy) = &self.legacy {
            return legacy.original_bytes().len();
        }
        8 + self.original_index_frame.len() + self.payload.len()
    }

    fn original_segments(&self) -> [&[u8]; 3] {
        if let Some(legacy) = &self.legacy {
            return [legacy.original_bytes(), &[], &[]];
        }
        [
            self.original_header.as_slice(),
            self.original_index_frame.as_slice(),
            self.payload.as_slice(),
        ]
    }

    /// Stable session identity; actual saving uses the full byte comparison.
    pub fn original_fingerprint(&self) -> String {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in self.original_segments().into_iter().flatten() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("fnv1a64:{hash:016x}:{:x}", self.original_len())
    }

    /// Compare with the complete source, rather than a timestamp or short hash.
    pub fn matches_original(&self, path: &Path) -> Result<bool> {
        let mut file = File::open(path).context("无法核对原 RP 文件")?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != self.original_len() as u64 {
            return Ok(false);
        }
        let mut buffer = [0u8; 64 * 1024];
        for segment in self.original_segments() {
            for chunk in segment.chunks(buffer.len()) {
                file.read_exact(&mut buffer[..chunk.len()])
                    .context("核对原 RP 文件时读取失败")?;
                if buffer[..chunk.len()] != *chunk {
                    return Ok(false);
                }
            }
        }
        let mut extra = [0];
        Ok(file.read(&mut extra)? == 0)
    }

    /// Replace decoded parts while retaining all untouched frames byte-for-byte.
    pub fn encode_with_parts(&self, replacements: &BTreeMap<u64, Vec<u8>>) -> Result<Vec<u8>> {
        self.encode_with_index_and_parts(&self.index, replacements)
    }

    /// Encode an edited index, optionally appending new parts.
    ///
    /// For a new part, use a unique offset absent from the original index (for
    /// example `u64::MAX - n`) in both `index` and `replacements`. Such virtual
    /// offsets are replaced with the final appended offsets. Removed index
    /// entries do not destroy the original part bytes.
    pub fn encode_with_index_and_parts(
        &self,
        index: &Value,
        replacements: &BTreeMap<u64, Vec<u8>>,
    ) -> Result<Vec<u8>> {
        if let Some(legacy) = &self.legacy {
            return legacy.encode_with_parts(index, replacements);
        }
        ensure!(
            index.get("root").is_some_and(Value::is_object),
            "原生索引缺少 root 对象"
        );
        if replacements.is_empty() && index == &self.original_index {
            return Ok(self.original_bytes());
        }
        let mut original_offsets = BTreeSet::new();
        collect_offsets(&self.original_index, &mut original_offsets)?;
        let mut requested_offsets = BTreeSet::new();
        collect_offsets(index, &mut requested_offsets)?;
        for offset in replacements.keys() {
            ensure!(
                original_offsets.contains(offset) || requested_offsets.contains(offset),
                "替换部件未在 原生索引中引用：{offset}"
            );
        }

        let mut payload = Vec::new();
        let mut relocated = BTreeMap::new();
        let mut cursor = 0usize;
        for offset in &original_offsets {
            let start = usize::try_from(*offset).context("原生部件偏移超出范围")?;
            ensure!(start >= cursor, "原生索引中的部件范围重叠");
            let end = self.part_end(start)?;
            append_bounded(
                &mut payload,
                self.payload
                    .get(cursor..start)
                    .context("原生部件偏移超出范围")?,
            )?;
            relocated.insert(*offset, payload.len() as u64);
            if let Some(decoded) = replacements.get(offset) {
                let frame = encode_part(decoded)?;
                append_bounded(&mut payload, &frame)?;
            } else {
                append_bounded(&mut payload, &self.payload[start..end])?;
            }
            cursor = end;
        }
        append_bounded(&mut payload, &self.payload[cursor..])?;
        for offset in requested_offsets.difference(&original_offsets) {
            let decoded = replacements
                .get(offset)
                .with_context(|| format!("原生新部件缺少数据：{offset}"))?;
            relocated.insert(*offset, payload.len() as u64);
            append_bounded(&mut payload, &encode_part(decoded)?)?;
        }
        let mut updated = index.clone();
        relocate_offsets(&mut updated, &relocated)?;
        let frame = encode_index(&updated)?;
        let total = 8usize
            .checked_add(frame.len())
            .and_then(|n| n.checked_add(payload.len()))
            .context("RP 文件长度溢出")?;
        ensure!(total <= MAX_FILE_BYTES, "RP 文件超过 256 MiB 写入上限");
        let mut bytes = Vec::with_capacity(total);
        bytes.extend_from_slice(&self.original_header[..4]);
        bytes.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&frame);
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }

    fn part_end(&self, start: usize) -> Result<usize> {
        let header_end = start.checked_add(8).context("原生部件偏移溢出")?;
        let header = self
            .payload
            .get(start..header_end)
            .context("原生部件头不完整")?;
        ensure!(
            u32::from_le_bytes(header[..4].try_into()?) == 0,
            "暂不支持此 原生部件类型"
        );
        let length = u32::from_le_bytes(header[4..8].try_into()?) as usize;
        ensure!(length <= MAX_PART_BYTES, "原生部件超过 64 MiB 读取上限");
        let end = header_end.checked_add(length).context("原生部件长度溢出")?;
        ensure!(end <= self.payload.len(), "原生部件数据不完整");
        Ok(end)
    }

    /// Save back only if the on-disk source still matches the opened document.
    pub fn write_atomic_checked(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        write_atomic_impl(path, bytes, Some(self))
    }
}

fn append_bounded(target: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    ensure!(
        target
            .len()
            .checked_add(bytes.len())
            .is_some_and(|n| n <= MAX_FILE_BYTES),
        "RP 文件超过 256 MiB 写入上限"
    );
    target.extend_from_slice(bytes);
    Ok(())
}

fn collect_offsets(node: &Value, offsets: &mut BTreeSet<u64>) -> Result<()> {
    if let Some(object) = node.as_object() {
        if let Some(parts) = object.get("parts") {
            for offset in parts
                .as_object()
                .context("原生 parts 索引不是对象")?
                .values()
            {
                offsets.insert(offset.as_u64().context("原生部件偏移不是无符号整数")?);
            }
        }
        if let Some(children) = object.get("children") {
            for child in children
                .as_object()
                .context("原生 children 索引不是对象")?
                .values()
            {
                collect_offsets(child, offsets)?;
            }
        }
        if let Some(root) = object.get("root") {
            collect_offsets(root, offsets)?;
        }
    }
    Ok(())
}

fn relocate_offsets(node: &mut Value, offsets: &BTreeMap<u64, u64>) -> Result<()> {
    if let Some(object) = node.as_object_mut() {
        if let Some(parts) = object.get_mut("parts") {
            for offset in parts
                .as_object_mut()
                .context("原生 parts 索引不是对象")?
                .values_mut()
            {
                let old = offset.as_u64().context("原生部件偏移不是无符号整数")?;
                *offset = Value::from(*offsets.get(&old).context("原生部件缺少重定位记录")?);
            }
        }
        if let Some(children) = object.get_mut("children") {
            for child in children
                .as_object_mut()
                .context("原生 children 索引不是对象")?
                .values_mut()
            {
                relocate_offsets(child, offsets)?;
            }
        }
        if let Some(root) = object.get_mut("root") {
            relocate_offsets(root, offsets)?;
        }
    }
    Ok(())
}

fn write_uleb128(mut value: usize, bytes: &mut Vec<u8>) {
    while value >= 128 {
        bytes.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    bytes.push(value as u8);
}

fn encode_index(index: &Value) -> Result<Vec<u8>> {
    let decoded = serde_json::to_vec(index).context("原生索引序列化失败")?;
    ensure!(
        decoded.len() <= MAX_PART_BYTES,
        "原生索引超过 64 MiB 写入上限"
    );
    let encoded = lz4_flex::block::compress(&decoded);
    let mut frame = vec![1];
    write_uleb128(decoded.len(), &mut frame);
    write_uleb128(encoded.len(), &mut frame);
    frame.extend_from_slice(&encoded);
    ensure!(
        frame.len() <= MAX_PART_BYTES,
        "原生索引压缩后超过 64 MiB 写入上限"
    );
    Ok(frame)
}

fn encode_part(decoded: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        decoded.len() <= MAX_PART_BYTES,
        "原生部件超过 64 MiB 写入上限"
    );
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(decoded).context("原生部件压缩失败")?;
    let encoded = encoder.finish().context("原生部件压缩失败")?;
    ensure!(
        encoded.len() <= MAX_PART_BYTES,
        "原生部件压缩后超过 64 MiB 写入上限"
    );
    let mut frame = Vec::with_capacity(8 + encoded.len());
    frame.extend_from_slice(&0u32.to_le_bytes());
    frame.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
    frame.extend_from_slice(&encoded);
    Ok(frame)
}

/// Atomically save a completed native container (for Save As).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_impl(path, bytes, None)
}

fn write_atomic_impl(path: &Path, bytes: &[u8], expected: Option<&Container>) -> Result<()> {
    ensure!(
        bytes.len() <= MAX_FILE_BYTES,
        "RP 文件超过 256 MiB 写入上限"
    );
    // Validate before creating any file, and retain the original on failure.
    from_bytes(bytes).context("待保存 RP 文件无效")?;
    if let Some(source) = expected {
        ensure!(
            source.matches_original(path)?,
            "原 RP 文件已被其他程序修改，请重新打开后再保存"
        );
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    ensure!(parent.is_dir(), "保存目录不存在：{}", parent.display());
    let name = path
        .file_name()
        .context("保存路径缺少文件名")?
        .to_string_lossy();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut temporary = None;
    for attempt in 0..16 {
        let candidate = parent.join(format!(
            ".{name}.{}.{nonce}.{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("无法创建临时 RP 文件"),
        }
    }
    let (temporary_path, mut file) = temporary.context("无法创建唯一临时 RP 文件")?;
    let result = (|| -> Result<()> {
        file.write_all(bytes).context("写入 RP 文件失败")?;
        file.sync_all().context("同步 RP 文件失败")?;
        drop(file);
        if let Some(source) = expected {
            ensure!(
                source.matches_original(path)?,
                "原 RP 文件已被其他程序修改，请重新打开后再保存"
            );
        }
        atomic_replace(&temporary_path, path).context("替换 RP 文件失败")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let mut source_wide: Vec<u16> = source.as_os_str().encode_wide().collect();
    let mut destination_wide: Vec<u16> = destination.as_os_str().encode_wide().collect();
    if source_wide.contains(&0) || destination_wide.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "路径包含 NUL 字符",
        ));
    }
    source_wide.push(0);
    destination_wide.push(0);
    let result = unsafe { MoveFileExW(source_wide.as_ptr(), destination_wide.as_ptr(), 0x1 | 0x8) };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

fn decode_index(frame: &[u8]) -> Result<Vec<u8>> {
    ensure!(!frame.is_empty(), "原生索引帧为空");
    ensure!(frame[0] == 1, "暂不支持 原生索引压缩类型 {}", frame[0]);
    let mut cursor = 1;
    let decoded_len = read_uleb128(frame, &mut cursor)?;
    let encoded_len = read_uleb128(frame, &mut cursor)?;
    ensure!(
        decoded_len <= MAX_PART_BYTES as u64,
        "原生索引解压后超过 64 MiB 上限"
    );
    ensure!(
        encoded_len <= MAX_PART_BYTES as u64,
        "原生索引压缩数据超过 64 MiB 上限"
    );
    let encoded_len = usize::try_from(encoded_len).context("原生索引长度超出范围")?;
    let end = cursor
        .checked_add(encoded_len)
        .context("原生索引长度溢出")?;
    ensure!(end == frame.len(), "原生索引压缩长度与帧长度不符");
    let mut decoded = vec![0; decoded_len as usize];
    let actual = lz4_flex::block::decompress_into(&frame[cursor..end], &mut decoded)
        .context("原生 LZ4 索引解压失败")?;
    ensure!(actual == decoded.len(), "原生索引解压长度不符");
    Ok(decoded)
}

fn read_uleb128(bytes: &[u8], cursor: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..=63).step_by(7) {
        let byte = *bytes.get(*cursor).context("原生索引长度字段不完整")?;
        *cursor += 1;
        if shift == 63 && byte > 1 {
            bail!("原生索引长度字段溢出");
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("原生索引长度字段溢出")
}

fn decode_part(data: &[u8], limit: usize) -> Result<Vec<u8>> {
    ensure!(data.len() <= limit, "原生部件超过解压上限");
    if !data.starts_with(&[0x1f, 0x8b]) {
        return Ok(data.to_vec());
    }
    let decoder = MultiGzDecoder::new(data);
    let mut decoded = Vec::new();
    decoder
        .take(limit as u64 + 1)
        .read_to_end(&mut decoded)
        .context("原生 Gzip 部件解压失败")?;
    ensure!(decoded.len() <= limit, "原生部件解压后超过 64 MiB 上限");
    Ok(decoded)
}

#[cfg(test)]
mod tests;
