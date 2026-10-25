use super::*;

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

impl Reader<'_> {
    pub(super) fn take(&mut self, len: usize) -> Result<&[u8]> {
        let end = self.cursor.checked_add(len).context("原生长度溢出")?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .with_context(|| format!("原生对象包在偏移 {} 处被截断", self.cursor))?;
        self.cursor = end;
        Ok(value)
    }

    pub(super) fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("fixed-size slice"))
    }

    pub(super) fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub(super) fn count(&mut self) -> Result<usize> {
        let value = self.i32()?;
        ensure!(value >= 0, "原生对象包包含负数长度");
        let count = value as usize;
        ensure!(count <= MAX_ITEMS, "原生对象包项目数量超过限制");
        Ok(count)
    }

    pub(super) fn length(&mut self) -> Result<usize> {
        let value = self.i32()?;
        ensure!(value >= 0, "原生对象包包含负数数据长度");
        let len = value as usize;
        ensure!(len <= MAX_BYTES, "原生对象包数据长度超过限制");
        Ok(len)
    }

    pub(super) fn expect(&mut self, expected: i32) -> Result<()> {
        let offset = self.cursor;
        let token = self.i32()?;
        ensure!(
            token == expected,
            "原生对象包偏移 {offset}：需要标记 {expected}，实际为 {token}"
        );
        Ok(())
    }

    pub(super) fn string(&mut self) -> Result<Option<String>> {
        let id = self.i32()?;
        ensure!(id >= 0, "原生字符串引用为负数");
        self.strings
            .get(id as usize)
            .cloned()
            .with_context(|| format!("原生字符串引用 {id} 超出表范围"))
    }

    pub(super) fn guid(&mut self) -> Result<String> {
        let id = self.i32()?;
        ensure!(id >= 0, "原生 GUID 引用为负数");
        self.guids
            .get(id as usize)
            .cloned()
            .with_context(|| format!("原生 GUID 引用 {id} 超出表范围"))
    }

    pub(super) fn graph(mut self) -> Result<Graph> {
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

    pub(super) fn value(&mut self, depth: usize) -> Result<(Value, ValueLayout)> {
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
