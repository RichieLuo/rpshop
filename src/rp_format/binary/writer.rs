use super::*;

impl<'a> Writer<'a> {
    pub(super) fn new(source: Option<&'a Encoding>) -> Self {
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

    pub(super) fn int(&mut self, value: i32) {
        push_int(&mut self.data, value);
    }

    pub(super) fn string(&mut self, text: Option<&str>) -> Result<()> {
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

    pub(super) fn guid(&mut self, guid: &str) -> Result<()> {
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

    pub(super) fn value(
        &mut self,
        value: &Value,
        layout: Option<&ValueLayout>,
        depth: usize,
    ) -> Result<()> {
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

    pub(super) fn child_value(
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

    pub(super) fn new_dictionary_key(
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

    pub(super) fn scalar_unchanged(&self, value: &Value, token: i32, span: &Range<usize>) -> bool {
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

    pub(super) fn scalar(&mut self, value: &Value, token: i32) -> Result<()> {
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
