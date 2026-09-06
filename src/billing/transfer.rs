//! 价格表 XML/JSON 导入导出（PLAN §5.6 v1.9）。契约 contracts/m5-billing.md §7，M5-B 实现。
//!
//! - `export_json` / `export_xml`：把聚合后的 `PriceExportItem` 序列化为契约格式；
//! - `detect_format`：按首非空白字符识别 `xml` / `json`；
//! - `parse_json` / `parse_xml`：把导入文件还原为 `Vec<PriceExportItem>`；
//! - 导入的逐行校验 / upsert 与上游名解析逻辑放 admin/pricing.rs（见契约 §8）。
//!
//! XML 扩展格式约定（自洽往返）：
//! - 四列 token 无分段时 `<input_per_m>3.0</input_per_m>`；有分段时
//!   `<input_per_m><price>3.0</price><segments><segment>..</segment></segments></input_per_m>`；
//! - `image` / `video` 为扩展节点，内含若干 `<item>`（base_price / dimension_key / dimensions / segments）；
//! - `segments` 字段在 `PriceExportItem` 中为 `{unit: [Segment,...]}`（按 unit 名映射），
//!   与 `Segment` 结构（billing::Segment）逐字段对应。

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::billing::Segment;
use crate::error::{ApiError, ApiResult};

/// 导出条目（一个 upstream×model×currency 的 token 四价 + 可选扩展）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PriceExportItem {
    pub model_id: String,
    /// upstream.name
    pub upstream: String,
    pub currency: String,
    pub input_per_m: Option<Decimal>,
    pub output_per_m: Option<Decimal>,
    pub cache_read_per_m: Option<Decimal>,
    pub cache_write_per_m: Option<Decimal>,
    /// 可选：{unit: [Segment,...]}
    pub segments: Option<serde_json::Value>,
    pub image: Option<serde_json::Value>,
    pub video: Option<serde_json::Value>,
}

/// JSON 导出的顶层对象（`schema` 字段按契约固定，解析时不强制校验）。
#[derive(Debug, Deserialize)]
struct ExportJson {
    #[serde(default)]
    #[allow(dead_code)]
    schema: Option<String>,
    prices: Vec<PriceExportItem>,
}

/// token 四列：XML 标签名 → 计价 unit 名（segments 映射用）。
const TOKEN_COLS: &[(&str, &str)] = &[
    ("input_per_m", "token_in"),
    ("output_per_m", "token_out"),
    ("cache_read_per_m", "token_cache_read"),
    ("cache_write_per_m", "token_cache_write"),
];

pub fn export_json(items: &[PriceExportItem]) -> serde_json::Value {
    serde_json::json!({ "schema": "nextapi-prices/v1", "prices": items })
}

/// XML 转义（5 个预定义实体；数字直接用 `to_string`，无需转义）。
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 单个分段 → `<segment>...</segment>`。
fn segment_xml(seg: &Segment) -> String {
    let mut s = String::from("<segment>");
    if let Some(name) = &seg.name {
        s.push_str(&format!("<name>{}</name>", xml_escape(name)));
    }
    s.push_str(&format!("<price>{}</price>", seg.price));
    if let Some(wds) = &seg.weekdays {
        s.push_str("<weekdays>");
        for d in wds {
            s.push_str(&format!("<day>{}</day>", xml_escape(d)));
        }
        s.push_str("</weekdays>");
    }
    if let Some(ws) = &seg.windows {
        s.push_str("<windows>");
        for (a, b) in ws {
            s.push_str(&format!(
                "<window start=\"{}\" end=\"{}\"/>",
                xml_escape(a),
                xml_escape(b)
            ));
        }
        s.push_str("</windows>");
    }
    if let Some(m) = seg.min_prompt_tokens {
        s.push_str(&format!("<min_prompt_tokens>{m}</min_prompt_tokens>"));
    }
    if let Some(m) = seg.max_prompt_tokens {
        s.push_str(&format!("<max_prompt_tokens>{m}</max_prompt_tokens>"));
    }
    s.push_str("</segment>");
    s
}

/// 把某 unit 的分段数组（serde_json::Value 中的数组）序列化为 `<segments>...</segments>`。
/// 解析失败/为空时返回空串（该列纯数字）。
fn segments_to_xml(seg_arr: &serde_json::Value) -> String {
    let segs: Vec<Segment> = serde_json::from_value(seg_arr.clone()).unwrap_or_default();
    if segs.is_empty() {
        return String::new();
    }
    let mut out = String::from("<segments>");
    for s in &segs {
        out.push_str(&segment_xml(s));
    }
    out.push_str("</segments>");
    out
}

/// 从 item.segments 取值（按 unit 名取分段数组）。
fn unit_segments<'a>(
    segments: Option<&'a serde_json::Value>,
    unit: &str,
) -> Option<&'a serde_json::Value> {
    segments.and_then(|m| m.get(unit))
}

/// 生成一个 token 列的 XML 节点；无价格时返回空串。
fn token_col_xml(
    tag: &str,
    unit: &str,
    price: Option<Decimal>,
    segments: Option<&serde_json::Value>,
) -> String {
    let Some(p) = price else {
        return String::new();
    };
    let seg_xml = unit_segments(segments, unit)
        .map(segments_to_xml)
        .unwrap_or_default();
    if seg_xml.is_empty() {
        format!("<{tag}>{p}</{tag}>")
    } else {
        format!("<{tag}><price>{p}</price>{seg_xml}</{tag}>")
    }
}

/// 图片/视频扩展节点 `<image>` / `<video>`，内含 `<item>`（base_price/dimension_key/dimensions/segments）。
fn media_xml(tag: &str, rules: Option<&serde_json::Value>) -> String {
    let Some(rules) = rules else {
        return String::new();
    };
    let arr = rules.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return String::new();
    }
    let mut out = format!("<{tag}>");
    for item in &arr {
        out.push_str("<item>");
        if let Some(p) = item.get("base_price") {
            out.push_str(&format!("<price>{}</price>", json_value_text(p)));
        }
        if let Some(dk) = item.get("dimension_key").and_then(|v| v.as_str()) {
            out.push_str(&format!(
                "<dimension_key>{}</dimension_key>",
                xml_escape(dk)
            ));
        }
        if let Some(dims) = item.get("dimensions") {
            out.push_str(&dimensions_xml(dims));
        }
        if let Some(seg) = item.get("segments") {
            let seg_xml = segments_to_xml(seg);
            if !seg_xml.is_empty() {
                out.push_str(&seg_xml);
            }
        }
        out.push_str("</item>");
    }
    out.push_str(&format!("</{tag}>"));
    out
}

/// 把 JSON 值转成 XML 文本：String 直接用其内容（不带引号），其它用 `to_string`。
fn json_value_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 维度对象序列化：`<dimensions><dimension name="..">value</dimension></dimensions>`。
fn dimensions_xml(dims: &serde_json::Value) -> String {
    let Some(obj) = dims.as_object() else {
        return String::new();
    };
    if obj.is_empty() {
        return String::new();
    }
    let mut out = String::from("<dimensions>");
    for (k, v) in obj {
        out.push_str(&format!(
            "<dimension name=\"{}\">{}</dimension>",
            xml_escape(k),
            xml_escape(&json_value_text(v))
        ));
    }
    out.push_str("</dimensions>");
    out
}

pub fn export_xml(items: &[PriceExportItem]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<prices schema=\"nextapi-prices/v1\">");
    for item in items {
        out.push_str("<price>");
        out.push_str(&format!(
            "<model_id>{}</model_id>",
            xml_escape(&item.model_id)
        ));
        out.push_str(&format!(
            "<upstream>{}</upstream>",
            xml_escape(&item.upstream)
        ));
        out.push_str(&format!(
            "<currency>{}</currency>",
            xml_escape(&item.currency)
        ));
        for (tag, unit) in TOKEN_COLS {
            let price = match *tag {
                "input_per_m" => item.input_per_m,
                "output_per_m" => item.output_per_m,
                "cache_read_per_m" => item.cache_read_per_m,
                _ => item.cache_write_per_m,
            };
            out.push_str(&token_col_xml(tag, unit, price, item.segments.as_ref()));
        }
        if let Some(img) = &item.image {
            out.push_str(&media_xml("image", Some(img)));
        }
        if let Some(vid) = &item.video {
            out.push_str(&media_xml("video", Some(vid)));
        }
        out.push_str("</price>");
    }
    out.push_str("</prices>");
    out
}

/// 首非空白字符 '<' → "xml"，'{' → "json"；其它回退 "json"。
pub fn detect_format(bytes: &[u8]) -> &'static str {
    for &b in bytes {
        match b {
            b' ' | b'\t' | b'\n' | b'\r' => continue,
            b'<' => return "xml",
            b'{' => return "json",
            _ => return "json",
        }
    }
    "json"
}

pub fn parse_json(bytes: &[u8]) -> ApiResult<Vec<PriceExportItem>> {
    let v: ExportJson = serde_json::from_slice(bytes)
        .map_err(|e| ApiError::bad_request(format!("JSON 解析失败: {e}")))?;
    Ok(v.prices)
}

// ---------------------------------------------------------------------------
// XML 解析：把 XML 解析成轻量树，再映射为 PriceExportItem。
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct XNode {
    name: String,
    attrs: HashMap<String, String>,
    children: Vec<XNode>,
    text: String,
}

/// 读取元素属性（解码实体）。
fn read_attrs(e: &quick_xml::events::BytesStart) -> ApiResult<HashMap<String, String>> {
    let mut attrs = HashMap::new();
    for attr in e.attributes() {
        let attr = attr.map_err(|err| ApiError::bad_request(format!("XML 属性解析失败: {err}")))?;
        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
        let value = attr
            .unescape_value()
            .map_err(|err| ApiError::bad_request(format!("XML 属性值解码失败: {err}")))?
            .into_owned();
        attrs.insert(key, value);
    }
    Ok(attrs)
}

/// 把整个 XML 解析成一棵 XNode 树。
fn parse_tree(bytes: &[u8]) -> ApiResult<XNode> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_reader(bytes);
    let mut stack: Vec<XNode> = Vec::new();
    let mut root: Option<XNode> = None;

    // 子节点入栈：栈空则设为根，否则挂到栈顶的 children。
    fn push_child(stack: &mut [XNode], root: &mut Option<XNode>, node: XNode) {
        if let Some(top) = stack.last_mut() {
            top.children.push(node);
        } else {
            *root = Some(node);
        }
    }

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let attrs = read_attrs(&e)?;
                stack.push(XNode {
                    name,
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                });
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let attrs = read_attrs(&e)?;
                let node = XNode {
                    name,
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                };
                push_child(&mut stack, &mut root, node);
            }
            Ok(Event::Text(e)) => {
                let t = e
                    .unescape()
                    .map_err(|err| ApiError::bad_request(format!("XML 文本解码失败: {err}")))?
                    .into_owned();
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t);
                }
            }
            Ok(Event::End(_)) => {
                if let Some(node) = stack.pop() {
                    push_child(&mut stack, &mut root, node);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(ApiError::bad_request(format!("XML 解析失败: {e}"))),
            _ => {}
        }
    }
    root.ok_or_else(|| ApiError::bad_request("XML 内容为空"))
}

/// 取节点下指定名称的第一个直接子节点。
fn child<'a>(node: &'a XNode, name: &str) -> Option<&'a XNode> {
    node.children.iter().find(|c| c.name == name)
}

/// 解析 `<segments>` 节点为 Vec<Segment>。
fn parse_segments(node: &XNode) -> Vec<Segment> {
    node.children
        .iter()
        .filter(|c| c.name == "segment")
        .map(parse_segment)
        .collect()
}

fn parse_segment(node: &XNode) -> Segment {
    let text_of = |name: &str| child(node, name).map(|c| c.text.trim().to_string());
    let price = text_of("price")
        .and_then(|s| s.parse::<Decimal>().ok())
        .unwrap_or(Decimal::ZERO);
    let weekdays = child(node, "weekdays").map(|w| {
        w.children
            .iter()
            .filter(|d| d.name == "day")
            .map(|d| d.text.trim().to_string())
            .collect()
    });
    let windows = child(node, "windows").map(|w| {
        w.children
            .iter()
            .filter(|x| x.name == "window")
            .map(|x| {
                (
                    x.attrs.get("start").cloned().unwrap_or_default(),
                    x.attrs.get("end").cloned().unwrap_or_default(),
                )
            })
            .collect()
    });
    Segment {
        name: text_of("name").filter(|s| !s.is_empty()),
        price,
        weekdays,
        windows,
        min_prompt_tokens: text_of("min_prompt_tokens").and_then(|s| s.parse::<i64>().ok()),
        max_prompt_tokens: text_of("max_prompt_tokens").and_then(|s| s.parse::<i64>().ok()),
    }
}

/// 解析 `<dimensions>` 节点为 JSON 对象。
fn parse_dimensions(node: &XNode) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for d in node.children.iter().filter(|c| c.name == "dimension") {
        if let Some(name) = d.attrs.get("name") {
            m.insert(name.clone(), serde_json::json!(d.text.trim()));
        }
    }
    serde_json::Value::Object(m)
}

/// 解析 token 列节点（可能含 `<price>` 或纯文本），返回 (价格, 可选分段数组)。
fn parse_token_col(node: &XNode) -> (Option<Decimal>, Option<serde_json::Value>) {
    let price = child(node, "price")
        .and_then(|p| p.text.trim().parse::<Decimal>().ok())
        .or_else(|| node.text.trim().parse::<Decimal>().ok());
    let segments = child(node, "segments").map(|s| {
        let segs = parse_segments(s);
        serde_json::to_value(segs).unwrap_or(serde_json::Value::Array(Vec::new()))
    });
    (price, segments)
}

/// 解析 `<image>` / `<video>` 节点为规则数组。
fn parse_media(node: &XNode) -> serde_json::Value {
    let mut arr = Vec::new();
    for item in node.children.iter().filter(|c| c.name == "item") {
        let mut m = serde_json::Map::new();
        if let Some(p) = child(item, "price") {
            if let Ok(d) = p.text.trim().parse::<Decimal>() {
                m.insert("base_price".into(), serde_json::json!(d));
            }
        }
        if let Some(dk) = child(item, "dimension_key") {
            m.insert("dimension_key".into(), serde_json::json!(dk.text.trim()));
        }
        if let Some(dims) = child(item, "dimensions") {
            m.insert("dimensions".into(), parse_dimensions(dims));
        }
        if let Some(segs) = child(item, "segments") {
            let v = serde_json::to_value(parse_segments(segs))
                .unwrap_or(serde_json::Value::Array(Vec::new()));
            m.insert("segments".into(), v);
        }
        arr.push(serde_json::Value::Object(m));
    }
    serde_json::Value::Array(arr)
}

pub fn parse_xml(bytes: &[u8]) -> ApiResult<Vec<PriceExportItem>> {
    let root = parse_tree(bytes)?;
    if root.name != "prices" {
        return Err(ApiError::bad_request("XML 根元素应为 <prices>"));
    }
    let mut items = Vec::new();
    for price in root.children.iter().filter(|c| c.name == "price") {
        let text_of = |name: &str| {
            child(price, name)
                .map(|c| c.text.trim().to_string())
                .unwrap_or_default()
        };
        let mut item = PriceExportItem {
            model_id: text_of("model_id"),
            upstream: text_of("upstream"),
            currency: text_of("currency"),
            ..Default::default()
        };
        let mut seg_map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        for (tag, unit) in TOKEN_COLS {
            if let Some(col) = child(price, tag) {
                let (price_val, segments) = parse_token_col(col);
                match *tag {
                    "input_per_m" => item.input_per_m = price_val,
                    "output_per_m" => item.output_per_m = price_val,
                    "cache_read_per_m" => item.cache_read_per_m = price_val,
                    _ => item.cache_write_per_m = price_val,
                }
                if let Some(seg) = segments {
                    if let Some(arr) = seg.as_array() {
                        if !arr.is_empty() {
                            seg_map.insert(unit.to_string(), seg);
                        }
                    }
                }
            }
        }
        if !seg_map.is_empty() {
            item.segments = Some(serde_json::Value::Object(seg_map));
        }
        if let Some(img) = child(price, "image") {
            let v = parse_media(img);
            if v.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                item.image = Some(v);
            }
        }
        if let Some(vid) = child(price, "video") {
            let v = parse_media(vid);
            if v.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                item.video = Some(v);
            }
        }
        items.push(item);
    }
    Ok(items)
}

// ---------------------------------------------------------------------------
// 单测（契约 §10）：导出→解析往返（xml/json 各一）、detect_format。
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn decimal(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    /// 构造一个带 token 四价 + 分段 + 图片/视频扩展的典型条目。
    /// 注意：本代码库中 `Decimal` 走 serde 默认实现序列化为**去尾零字符串**（如 3.0 → "3"、
    /// 0.5 → "0.5"），因此嵌套的分段价格 / base_price 用字符串形式，确保 XML/JSON 往返一致。
    fn sample_item() -> PriceExportItem {
        let segments = serde_json::json!({
            "token_in": [
                { "name": "peak", "price": "3.0",
                  "weekdays": ["mon", "tue"],
                  "windows": [["09:00", "12:00"], ["23:00", "02:00"]],
                  "min_prompt_tokens": 32769, "max_prompt_tokens": 40000 }
            ]
        });
        PriceExportItem {
            model_id: "deepseek-chat".into(),
            upstream: "dashscope".into(),
            currency: "CNY".into(),
            input_per_m: Some(decimal("3.0")),
            output_per_m: Some(decimal("6.0")),
            cache_read_per_m: Some(decimal("0.6")),
            cache_write_per_m: Some(decimal("3.0")),
            segments: Some(segments),
            image: Some(serde_json::json!([
                { "base_price": "2.0", "dimension_key": "{\"image_size\":\"1024x1024\"}",
                  "dimensions": { "image_size": "1024x1024" } }
            ])),
            video: Some(serde_json::json!([
                { "base_price": "0.5", "dimension_key": "{\"resolution\":\"720p\",\"task_type\":\"txt2vid\"}",
                  "dimensions": { "resolution": "720p", "task_type": "txt2vid" } }
            ])),
        }
    }

    #[test]
    fn json_roundtrip() {
        let items = vec![sample_item()];
        let json = export_json(&items);
        let bytes = serde_json::to_vec(&json).unwrap();
        assert_eq!(detect_format(&bytes), "json");
        let parsed = parse_json(&bytes).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], items[0]);
    }

    #[test]
    fn xml_roundtrip() {
        let items = vec![sample_item()];
        let xml = export_xml(&items);
        assert_eq!(detect_format(xml.as_bytes()), "xml");
        let parsed = parse_xml(xml.as_bytes()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], items[0]);
    }

    #[test]
    fn detect_format_variants() {
        assert_eq!(detect_format(b"<prices/>"), "xml");
        assert_eq!(detect_format(b"   \n{\"a\":1}"), "json");
        assert_eq!(detect_format(b"{\"prices\":[]}"), "json");
        // 其它字符回退 json
        assert_eq!(detect_format(b"abc"), "json");
        assert_eq!(detect_format(b"   "), "json");
    }

    #[test]
    fn xml_escapes_control_chars() {
        let mut item = sample_item();
        item.upstream = "a<b&c>\"d\"".into();
        let xml = export_xml(std::slice::from_ref(&item));
        let parsed = parse_xml(xml.as_bytes()).unwrap();
        assert_eq!(parsed[0].upstream, "a<b&c>\"d\"");
    }
}
