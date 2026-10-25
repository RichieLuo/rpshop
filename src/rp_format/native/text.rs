use super::*;

pub(super) fn set_text(properties: &mut Value, text: &str) {
    if properties.get("TextBlock").is_some() {
        properties["TextBlock"]["text"] = json!(text);
    } else {
        let field = if properties.get("rtf-block").is_some() {
            "rtf-block"
        } else {
            "RichTextBlock"
        };
        let mut paragraph=properties.pointer(&format!("/{field}/attributed-string/Paragraphs/0")).cloned()
            .unwrap_or_else(||json!({"Wrapping":0,"Trimming":0,"HorizontalAlignment":0,"LineSpacing":0.0,"ListInfo":{"IndentLevel":0,"ListType":0}}));
        let mut inline = paragraph.pointer("/Inlines/0").cloned().unwrap_or_else(|| {
            json!({
            "Type":"Axure:AttributedString:AxRun","Family":"Arial","Size":9.75,"Weight":400,
            "Style":0,"Stretch":5,"Underline":false,"Color":-13421773,"Baseline":0,
            "CharacterSpacing":0.0,"Strikethrough":false})
        });
        let paragraphs = text
            .split('\n')
            .map(|line| {
                inline["Text"] = json!(line);
                paragraph["Inlines"] = json!([inline.clone()]);
                paragraph.clone()
            })
            .collect::<Vec<_>>();
        if !properties[field].is_object() {
            properties[field] = json!({"*type":"rich-text-block","attributed-string":{
                "Version":{"Major":11,"Minor":0,"Build":0,"Revision":0},"Paragraphs":[]}});
        }
        properties[field]["attributed-string"]["Paragraphs"] = json!(paragraphs);
    }
}

pub(super) fn update_inlines(properties: &mut Value, widget: &Widget, scale: f32) -> Result<()> {
    let field = if properties.get("rtf-block").is_some() {
        "rtf-block"
    } else {
        "RichTextBlock"
    };
    if let Some(paragraphs) = properties
        .pointer_mut(&format!("/{field}/attributed-string/Paragraphs"))
        .and_then(Value::as_array_mut)
    {
        let color = color_argb(&widget.text_color)?;
        for paragraph in paragraphs {
            if let Some(inlines) = paragraph["Inlines"].as_array_mut() {
                for inline in inlines {
                    inline["Size"] = json!(widget.font_size / scale * 0.75);
                    inline["Color"] = json!(color);
                }
            }
        }
    }
    Ok(())
}

pub(super) fn apply_added_text_layout(properties: &mut Value, widget: &Widget) {
    let Some(layout) = &widget.text_layout else {
        return;
    };
    properties["AutoFitWidth"] = json!(!layout.wrap);
    let field = if properties.get("rtf-block").is_some() {
        "rtf-block"
    } else {
        "RichTextBlock"
    };
    if let Some(paragraphs) = properties
        .pointer_mut(&format!("/{field}/attributed-string/Paragraphs"))
        .and_then(Value::as_array_mut)
    {
        for paragraph in paragraphs {
            paragraph["HorizontalAlignment"] = json!(layout.horizontal);
            if let Some(inlines) = paragraph["Inlines"].as_array_mut() {
                for inline in inlines {
                    inline["Weight"] = json!(if layout.bold { 700 } else { 400 });
                }
            }
        }
    }
}

pub(super) fn contained_text_id(graph: &Graph, node: &Node) -> Option<String> {
    array(&node.properties["diagram-objects"])
        .iter()
        .filter_map(reference_id)
        .find(|id| {
            graph
                .node(id)
                .is_some_and(|n| n.kind.ends_with(":RichTextPanel"))
        })
        .map(str::to_owned)
}
