use super::*;
use flate2::{Compression, write::GzEncoder};
use std::io::Write;

fn varint(mut n: usize, out: &mut Vec<u8>) {
    while n >= 128 {
        out.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    out.push(n as u8);
}

// Literal-only LZ4 is deliberately constructed without the decoder's
// companion compressor, so framing tests have an independent fixture.
fn literal_lz4(data: &[u8]) -> Vec<u8> {
    let mut bytes = vec![(data.len().min(15) as u8) << 4];
    if data.len() >= 15 {
        let mut extra = data.len() - 15;
        while extra >= 255 {
            bytes.push(255);
            extra -= 255;
        }
        bytes.push(extra as u8);
    }
    bytes.extend_from_slice(data);
    bytes
}

fn container_bytes(index: &[u8], payload: &[u8]) -> Vec<u8> {
    let compressed = literal_lz4(index);
    let mut frame = vec![1];
    varint(index.len(), &mut frame);
    varint(compressed.len(), &mut frame);
    frame.extend_from_slice(&compressed);
    let mut bytes = vec![0xac, 0xef, 11, 0];
    bytes.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&frame);
    bytes.extend_from_slice(payload);
    bytes
}

fn part_bytes(data: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 4];
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(data);
    bytes
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn reads_literal_lz4_index_and_raw_gzip_parts() {
    let raw = [0xff, 0xd8, 0xff, 0xe0, 1, 2, 3];
    let mut payload = part_bytes(&raw);
    let second_offset = payload.len();
    payload.extend(part_bytes(&gzip(b"serialized project content")));
    let index =
        format!(r#"{{"root":{{"parts":{{"Thumbnail":0,"DesignDocument":{second_offset}}}}}}}"#);
    let container = from_bytes(&container_bytes(index.as_bytes(), &payload)).unwrap();
    assert_eq!(container.version, 11);
    assert_eq!(container.index["root"]["parts"]["Thumbnail"], 0);
    assert_eq!(container.part(0).unwrap(), raw);
    assert_eq!(
        container.part(second_offset as u64).unwrap(),
        b"serialized project content"
    );
}

#[test]
fn rejects_bad_headers_versions_and_truncated_frames() {
    let good = container_bytes(br#"{"root":{}}"#, &[]);
    for length in 0..good.len() {
        assert!(
            from_bytes(&good[..length]).is_err(),
            "accepted {length}-byte truncation"
        );
    }
    let mut bytes = good.clone();
    bytes[0] = 0;
    assert!(from_bytes(&bytes).is_err());
    let mut bytes = good.clone();
    bytes[2] = 12;
    assert!(from_bytes(&bytes).is_err());
    let mut bytes = good.clone();
    bytes[8] = 0;
    assert!(from_bytes(&bytes).is_err());
    let mut bytes = good;
    bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(from_bytes(&bytes).is_err());
}

#[test]
fn rejects_bad_index_lengths_lz4_json_and_root() {
    assert!(decode_index(&[1, 0xff]).is_err());
    let mut overflow = vec![1];
    overflow.extend([0xff; 10]);
    assert!(decode_index(&overflow).is_err());
    let mut bomb = vec![1];
    varint(MAX_PART_BYTES + 1, &mut bomb);
    varint(0, &mut bomb);
    assert!(decode_index(&bomb).is_err());
    assert!(decode_index(&[1, 12, 1, 0]).is_err());
    assert!(decode_index(&[1, 0, 1, 0, 0]).is_err());
    assert!(from_bytes(&container_bytes(b"not json", &[])).is_err());
    assert!(from_bytes(&container_bytes(b"[]", &[])).is_err());
    assert!(from_bytes(&container_bytes(br#"{"root":null}"#, &[])).is_err());
}

#[test]
fn rejects_invalid_part_offsets_tags_lengths_and_truncation() {
    let index = br#"{"root":{}}"#;
    let container = from_bytes(&container_bytes(index, &part_bytes(b"abc"))).unwrap();
    assert!(container.part(u64::MAX).is_err());
    assert!(container.part(1).is_err());
    assert!(container.part(4).is_err());
    let mut payload = part_bytes(b"abc");
    payload[0] = 1;
    assert!(
        from_bytes(&container_bytes(index, &payload))
            .unwrap()
            .part(0)
            .is_err()
    );
    payload[0] = 0;
    payload[4..8].copy_from_slice(&((MAX_PART_BYTES + 1) as u32).to_le_bytes());
    assert!(
        from_bytes(&container_bytes(index, &payload))
            .unwrap()
            .part(0)
            .is_err()
    );
    assert!(
        from_bytes(&container_bytes(index, &part_bytes(b"abc")[..10]))
            .unwrap()
            .part(0)
            .is_err()
    );
}

#[test]
fn bounds_gzip_expansion_and_checks_corruption() {
    let compressed = gzip(&vec![b'x'; 2048]);
    assert!(decode_part(&compressed, 1024).is_err());
    assert_eq!(decode_part(&compressed, 2048).unwrap().len(), 2048);
    assert!(decode_part(&compressed[..compressed.len() - 1], 4096).is_err());
    let mut corrupt = compressed.clone();
    let crc_start = corrupt.len() - 8;
    corrupt[crc_start] ^= 1;
    assert!(decode_part(&corrupt, 4096).is_err());
    let mut multiple = compressed;
    multiple.extend(gzip(b"second member"));
    assert_eq!(decode_part(&multiple, 4096).unwrap().len(), 2048 + 13);
    assert!(decode_part(b"too long", 1).is_err());
}

fn temporary_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("rpshop-native-{}-{nonce}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    directory
}

#[test]
fn unchanged_document_is_byte_exact_and_replacements_relocate_every_reference() {
    let mut payload = vec![0xaa, 0xbb];
    let first = payload.len() as u64;
    payload.extend(part_bytes(b"old"));
    payload.extend([0xcc, 0xdd]);
    let second = payload.len() as u64;
    let untouched = part_bytes(&[0xff, 0xd8, 0xff, 0xe0, 1, 2, 3]);
    payload.extend_from_slice(&untouched);
    let third = payload.len() as u64;
    payload.extend(part_bytes(&gzip(b"untouched compressed part")));
    payload.extend([0xfa, 0xfb]);
    let index = serde_json::json!({
        "root": {"parts": {"DesignDocument":first,"Alias":first},
            "children": {"page": {"parts":{"package":second},
                "children":{"nested":{"parts":{"package":third}}}}}},
        "unknownMetadata":{"preserve":"yes"}
    });
    let bytes = container_bytes(&serde_json::to_vec(&index).unwrap(), &payload);
    let source = from_bytes(&bytes).unwrap();
    assert_eq!(source.encode_with_parts(&BTreeMap::new()).unwrap(), bytes);
    let replacement = b"changed serialized package with more data".to_vec();
    let encoded = source
        .encode_with_parts(&BTreeMap::from([(first, replacement.clone())]))
        .unwrap();
    let reopened = from_bytes(&encoded).unwrap();
    let new_second = reopened.index["root"]["children"]["page"]["parts"]["package"]
        .as_u64()
        .unwrap();
    let new_third =
        reopened.index["root"]["children"]["page"]["children"]["nested"]["parts"]["package"]
            .as_u64()
            .unwrap();
    assert_eq!(reopened.index["root"]["parts"]["Alias"], first);
    assert_ne!(new_second, second);
    assert_eq!(new_third - new_second, third - second);
    assert_eq!(reopened.part(first).unwrap(), replacement);
    assert_eq!(
        reopened.part(new_second).unwrap(),
        source.part(second).unwrap()
    );
    assert_eq!(
        reopened.part(new_third).unwrap(),
        source.part(third).unwrap()
    );
    assert_eq!(
        &reopened.payload[new_second as usize..new_second as usize + untouched.len()],
        &untouched
    );
    assert_eq!(
        &reopened.payload[new_third as usize..],
        &source.payload[third as usize..]
    );
    assert_eq!(&reopened.payload[..2], &[0xaa, 0xbb]);
    assert_eq!(reopened.index["unknownMetadata"], index["unknownMetadata"]);
}

#[test]
fn added_parts_get_real_offsets_and_removed_entries_retain_original_bytes() {
    let old = part_bytes(b"retained original package");
    let index = serde_json::json!({"root":{"parts":{"DesignDocument":0},"children":{}}});
    let source = from_bytes(&container_bytes(&serde_json::to_vec(&index).unwrap(), &old)).unwrap();
    let mut edited = index;
    edited["root"]["children"]["new-page"] = serde_json::json!({"parts":{"package":u64::MAX}});
    let encoded = source
        .encode_with_index_and_parts(
            &edited,
            &BTreeMap::from([(u64::MAX, b"new package".to_vec())]),
        )
        .unwrap();
    let reopened = from_bytes(&encoded).unwrap();
    let added = reopened.index["root"]["children"]["new-page"]["parts"]["package"]
        .as_u64()
        .unwrap();
    assert_eq!(added, old.len() as u64);
    assert_eq!(reopened.part(added).unwrap(), b"new package");
    assert_eq!(&reopened.payload[..old.len()], &old);
    edited["root"]["parts"].as_object_mut().unwrap().clear();
    let encoded = source
        .encode_with_index_and_parts(
            &edited,
            &BTreeMap::from([(u64::MAX, b"new package".to_vec())]),
        )
        .unwrap();
    assert_eq!(&from_bytes(&encoded).unwrap().payload[..old.len()], &old);
    assert!(
        source
            .encode_with_parts(&BTreeMap::from([(42, Vec::new())]))
            .is_err()
    );
    assert!(
        source
            .encode_with_index_and_parts(&edited, &BTreeMap::new())
            .is_err()
    );
}

#[test]
fn native_atomic_save_preserves_destination_on_invalid_data_or_source_change() {
    let directory = temporary_directory();
    let path = directory.join("native.rp");
    let bytes = container_bytes(
        br#"{"root":{"parts":{"DesignDocument":0}}}"#,
        &part_bytes(b"original"),
    );
    write_atomic(&path, &bytes).unwrap();
    let source = open(&path).unwrap();
    assert!(source.matches_original(&path).unwrap());
    assert_eq!(
        source.original_fingerprint(),
        open(&path).unwrap().original_fingerprint()
    );
    assert!(write_atomic(&path, b"invalid native file").is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let updated = source
        .encode_with_parts(&BTreeMap::from([(0, b"modified".to_vec())]))
        .unwrap();
    // An external edit with unchanged length must still prevent overwrite.
    let mut external = bytes.clone();
    *external.last_mut().unwrap() ^= 1;
    fs::write(&path, &external).unwrap();
    assert!(!source.matches_original(&path).unwrap());
    assert_ne!(
        source.original_fingerprint(),
        open(&path).unwrap().original_fingerprint()
    );
    assert!(source.write_atomic_checked(&path, &updated).is_err());
    assert_eq!(fs::read(&path).unwrap(), external);
    assert_eq!(
        fs::read_dir(&directory).unwrap().count(),
        1,
        "failed save must remove temporary files"
    );
    fs::write(&path, &bytes).unwrap();
    source.write_atomic_checked(&path, &updated).unwrap();
    assert_eq!(open(&path).unwrap().part(0).unwrap(), b"modified");
    fs::remove_file(path).unwrap();
    fs::remove_dir(directory).unwrap();
}

#[test]
fn reads_installed_rp11_examples_when_present() {
    let base = Path::new(r"D:\Program Files (x86)\Axure\Axure RP 11\DefaultSettings");
    if !base.exists() {
        return;
    }
    fn check_parts(container: &Container, node: &Value) {
        if let Some(object) = node.as_object() {
            if let Some(parts) = object.get("parts").and_then(Value::as_object) {
                for offset in parts.values() {
                    assert!(!container.part(offset.as_u64().unwrap()).unwrap().is_empty());
                }
            }
            for child in object.values() {
                check_parts(container, child);
            }
        }
    }
    for name in [
        "Training/Prototype Starter.rp",
        "Training/Prototyping Basics.rp",
        "Training/Quick Win.rp",
        "Training/UX Prototyping.rp",
        "Libraries/Default.rplib",
        "Libraries/Flow.rplib",
        "Libraries/Icons.rplib",
        "Libraries/Sample form patterns.rplib",
        "Libraries/Sample UI patterns.rplib",
    ] {
        let path = base.join(name);
        if path.exists() {
            let container = open(&path).unwrap();
            assert_eq!(container.version, 11);
            check_parts(&container, &container.index);
        }
    }
}

#[test]
fn rewrites_installed_examples_only_to_temporary_files() {
    let base = Path::new(r"D:\Program Files (x86)\Axure\Axure RP 11\DefaultSettings");
    if !base.exists() {
        return;
    }
    let directory = temporary_directory();
    for name in [
        "Training/Prototype Starter.rp",
        "Training/Prototyping Basics.rp",
        "Training/Quick Win.rp",
        "Training/UX Prototyping.rp",
        "Libraries/Default.rplib",
        "Libraries/Flow.rplib",
        "Libraries/Icons.rplib",
        "Libraries/Sample form patterns.rplib",
        "Libraries/Sample UI patterns.rplib",
    ] {
        let installed_path = base.join(name);
        if !installed_path.exists() {
            continue;
        }
        let source = open(&installed_path).unwrap();
        let mut offsets = BTreeSet::new();
        collect_offsets(&source.index, &mut offsets).unwrap();
        // Recompress one serialized package with identical decoded content.
        let offset = source.index["root"]["parts"]["DesignDocument"]
            .as_u64()
            .unwrap_or_else(|| *offsets.first().unwrap());
        let encoded = source
            .encode_with_parts(&BTreeMap::from([(offset, source.part(offset).unwrap())]))
            .unwrap();
        let temporary_path = directory.join(installed_path.file_name().unwrap());
        write_atomic(&temporary_path, &encoded).unwrap();
        let reopened = open(&temporary_path).unwrap();
        let mut relocated = BTreeMap::new();
        fn pair_offsets(old: &Value, new: &Value, pairs: &mut BTreeMap<u64, u64>) {
            if let Some(object) = old.as_object() {
                if let Some(parts) = object.get("parts").and_then(Value::as_object) {
                    for (name, offset) in parts {
                        pairs.insert(
                            offset.as_u64().unwrap(),
                            new["parts"][name].as_u64().unwrap(),
                        );
                    }
                }
                if let Some(children) = object.get("children").and_then(Value::as_object) {
                    for (name, child) in children {
                        pair_offsets(child, &new["children"][name], pairs);
                    }
                }
                if let Some(root) = object.get("root") {
                    pair_offsets(root, &new["root"], pairs);
                }
            }
        }
        pair_offsets(&source.index, &reopened.index, &mut relocated);
        for old_offset in offsets {
            let new_offset = relocated[&old_offset];
            assert_eq!(
                reopened.part(new_offset).unwrap(),
                source.part(old_offset).unwrap()
            );
            if old_offset != offset {
                let original_frame = &source.payload
                    [old_offset as usize..source.part_end(old_offset as usize).unwrap()];
                let rewritten_frame = &reopened.payload
                    [new_offset as usize..reopened.part_end(new_offset as usize).unwrap()];
                assert_eq!(rewritten_frame, original_frame);
            }
        }
        assert!(
            source.matches_original(&installed_path).unwrap(),
            "installed fixture must remain untouched"
        );
        fs::remove_file(temporary_path).unwrap();
    }
    fs::remove_dir(directory).unwrap();
}
