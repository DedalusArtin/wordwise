//! 通用 JSON 路径取值器。
//!
//! 让自定义词典源无需写代码即可做字段映射（需求 6）。
//! 支持语法：
//! - `data.entries`      逐层向下
//! - `data.list[0].mean` 数组下标
//! - `data.list[*].mean` 遍历数组并收集为列表
//! - `$`                 返回整个根对象

use serde_json::Value;

/// 按路径表达式从 JSON 中取值。
pub fn query<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    if path == "$" {
        return Some(root);
    }

    let mut cur = root;
    for seg in split_path(path) {
        cur = step(cur, &seg)?;
    }
    Some(cur)
}

/// 按路径取值为字符串（数字/布尔也会转成字符串）。
pub fn query_str(root: &Value, path: &str) -> String {
    match query(root, path) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// 按路径取值为字符串列表。
///
/// 若路径末端命中数组，则把数组内元素都转成字符串收进来；
/// 若命中单个标量，则返回单元素列表。
///
/// 注意：这里走 `query_wildcard` 而不是 `query`。音标 / 音频这类字段
/// 常用 `[0].phonetics[*].audio` 这种「先展开数组再取字段」的写法，
/// 而 `query` 的 `[*]` 只做恒等传递、无法继续向数组元素下钻，
/// 用它会让 `[*]` 之后的字段整体取不到值。
pub fn query_list(root: &Value, path: &str) -> Vec<String> {
    let mut out = Vec::new();
    for v in query_wildcard(root, path) {
        match v {
            // 末端仍是数组时同样展开（例如 `a.b` 中 b 恰好是数组）
            Value::Array(arr) => {
                for item in arr {
                    push_scalar(item, &mut out);
                }
            }
            other => push_scalar(other, &mut out),
        }
    }
    out
}

fn push_scalar(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            if !s.trim().is_empty() {
                out.push(s.clone());
            }
        }
        Value::Number(n) => out.push(n.to_string()),
        Value::Bool(b) => out.push(b.to_string()),
        _ => {}
    }
}

/// 按路径取值为对象列表（用于 senses / examples 这类嵌套结构）。
pub fn query_objects<'a>(root: &'a Value, path: &str) -> Vec<&'a Value> {
    match query(root, path) {
        Some(Value::Array(arr)) => arr.iter().collect(),
        Some(v @ Value::Object(_)) => vec![v],
        _ => Vec::new(),
    }
}

/// 把 `a.b[0].c` 拆成 ["a", "b", "[0]", "c"]。
///
/// 规则：
/// - `.` 作为分隔符（括号外的点）
/// - `[...]` 整体作为一个段（含方括号），内部允许 `*` 或数字下标
/// - `a[0]` 这种紧贴写法会拆成 "a" 与 "[0]" 两段
fn split_path(path: &str) -> Vec<String> {
    let mut segs = Vec::new();
    let mut buf = String::new();
    let mut in_bracket = false;

    for ch in path.chars() {
        match ch {
            '[' => {
                // 遇到左括号：先收尾当前段，再开始括号段
                if !buf.is_empty() {
                    segs.push(std::mem::take(&mut buf));
                }
                in_bracket = true;
                buf.push('[');
            }
            ']' => {
                if in_bracket {
                    buf.push(']');
                    in_bracket = false;
                    segs.push(std::mem::take(&mut buf));
                }
                // 括号外的 `]` 属非法输入，直接忽略
            }
            '.' => {
                if in_bracket {
                    // 括号内的点按普通字符处理
                    buf.push('.');
                } else if !buf.is_empty() {
                    segs.push(std::mem::take(&mut buf));
                }
            }
            _ => buf.push(ch),
        }
    }
    if !buf.is_empty() {
        segs.push(buf);
    }
    segs
}

/// 走一步。
fn step<'a>(cur: &'a Value, seg: &str) -> Option<&'a Value> {
    if seg.starts_with('[') && seg.ends_with(']') {
        let inner = &seg[1..seg.len() - 1];
        if inner == "*" {
            return Some(cur);
        }
        let idx: usize = inner.parse().ok()?;
        return cur.as_array()?.get(idx);
    }
    cur.get(seg)
}

/// 通配取值：处理 `a[*].b` 这种「先展开数组再取字段」的情况。
/// 返回所有匹配到的值。
pub fn query_wildcard<'a>(root: &'a Value, path: &str) -> Vec<&'a Value> {
    let segs = split_path(path);
    let mut current: Vec<&Value> = vec![root];

    for seg in &segs {
        let mut next: Vec<&Value> = Vec::new();
        for c in &current {
            if seg.starts_with('[') && seg.ends_with(']') {
                let inner = &seg[1..seg.len() - 1];
                if inner == "*" {
                    if let Some(arr) = c.as_array() {
                        for v in arr {
                            next.push(v);
                        }
                    }
                } else if let Ok(i) = inner.parse::<usize>() {
                    if let Some(v) = c.as_array().and_then(|a| a.get(i)) {
                        next.push(v);
                    }
                }
            } else if let Some(v) = c.get(seg.as_str()) {
                next.push(v);
            }
        }
        current = next;
        if current.is_empty() {
            break;
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn simple_path() {
        let v = json!({"data": {"word": "apple"}});
        assert_eq!(query_str(&v, "data.word"), "apple");
    }

    #[test]
    fn array_index() {
        let v = json!({"list": [{"m": "first"}, {"m": "second"}]});
        assert_eq!(query_str(&v, "list[1].m"), "second");
    }

    #[test]
    fn wildcard_collect() {
        let v = json!({"list": [{"m": "a"}, {"m": "b"}]});
        let got: Vec<String> = query_wildcard(&v, "list[*].m")
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        assert_eq!(got, vec!["a", "b"]);
    }

    #[test]
    fn query_list_from_array() {
        let v = json!({"syn": ["big", "large"]});
        assert_eq!(query_list(&v, "syn"), vec!["big", "large"]);
    }

    #[test]
    fn query_list_single_scalar() {
        let v = json!({"syn": "big"});
        assert_eq!(query_list(&v, "syn"), vec!["big"]);
    }

    #[test]
    fn missing_path_returns_none() {
        let v = json!({"a": 1});
        assert!(query(&v, "b.c").is_none());
    }

    #[test]
    fn root_path() {
        let v = json!({"a": 1});
        // `$` 返回整个根对象：query 应命中，query_str 对对象返回空串
        assert!(query(&v, "$").is_some());
        assert!(query(&v, "$").unwrap().is_object());
        assert_eq!(query_str(&v, "$"), "");
    }

    #[test]
    fn split_path_handles_brackets() {
        assert_eq!(split_path("a.b[0].c"), vec!["a", "b", "[0]", "c"]);
        assert_eq!(split_path("list[*].m"), vec!["list", "[*]", "m"]);
        assert_eq!(split_path("a[1][2]"), vec!["a", "[1]", "[2]"]);
        assert_eq!(split_path("$"), vec!["$"]);
        assert_eq!(split_path("data.entries"), vec!["data", "entries"]);
    }
}
