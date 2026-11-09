//! Shapes Revolut answers into the compact JSON the tools return: empty values dropped, and lists
//! labelled with how many items they hold.

use serde_json::{Map, Value, json};

/// Removes nulls, empty strings and empty arrays or objects, recursively. `None` when nothing is
/// left. `false` and `0` are kept: they carry meaning in banking.
pub fn prune(v: Value) -> Option<Value> {
    match v {
        Value::Null => None,
        Value::String(s) if s.trim().is_empty() => None,
        Value::Array(items) => {
            let items: Vec<Value> = items.into_iter().filter_map(prune).collect();
            (!items.is_empty()).then_some(Value::Array(items))
        }
        Value::Object(map) => {
            let map: Map<String, Value> = map
                .into_iter()
                .filter_map(|(k, v)| prune(v).map(|v| (k, v)))
                .collect();
            (!map.is_empty()).then_some(Value::Object(map))
        }
        other => Some(other),
    }
}

/// One resource for output: empty values dropped.
pub fn resource(v: Value) -> Value {
    prune(v).unwrap_or_else(|| json!({}))
}

/// The items of a list answer: the array itself, or the array under `key` when Revolut wraps it.
pub fn items(answer: Value, key: &str) -> Vec<Value> {
    match answer {
        Value::Array(items) => items,
        Value::Object(mut map) => match map.remove(key) {
            Some(Value::Array(items)) => items,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
    .into_iter()
    .map(resource)
    .collect()
}

/// A list for output: `items` and `count`, and `more` with a hint when the list was cut short.
pub fn list(items: Vec<Value>, more: Option<String>) -> Value {
    let mut out = json!({"count": items.len(), "items": items});
    if let Some(hint) = more {
        out["more"] = Value::from(true);
        out["next"] = Value::from(hint);
    }
    out
}

/// Fields left out of every tool result. Revolut does not return full card numbers or CVVs to
/// these endpoints; these names are dropped anyway, in case it ever does.
const LEFT_OUT: [&str; 4] = ["pan", "cvv", "expiry", "card_number"];

/// Removes, recursively, what a tool result should not carry.
pub fn for_output(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for k in LEFT_OUT {
                map.remove(k);
            }
            map.values_mut().for_each(for_output);
        }
        Value::Array(items) => items.iter_mut().for_each(for_output),
        _ => {}
    }
}

/// An amount in minor units as a two-decimal string. Revolut gives amounts as JSON numbers in
/// major units, so totals are summed in minor units to stay exact.
pub fn minor(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    let s = match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().to_string(),
        _ => return None,
    };
    let neg = s.starts_with('-');
    let body = s.trim_start_matches('-');
    let (whole, frac) = body.split_once('.').unwrap_or((body, ""));
    if whole.is_empty() && frac.is_empty() {
        return None;
    }
    let whole: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let mut f: String = frac.chars().take(2).collect();
    while f.len() < 2 {
        f.push('0');
    }
    let f: i64 = f.parse().ok()?;
    let n = whole.checked_mul(100)?.checked_add(f)?;
    Some(if neg { -n } else { n })
}

/// Minor units as a decimal string with two places.
pub fn money(minor: i64) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    let m = minor.unsigned_abs();
    format!("{sign}{}.{:02}", m / 100, m % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_values_go_but_zero_and_false_stay() {
        let v = resource(
            json!({"a": null, "b": "", "c": [], "d": {}, "e": 0, "f": false, "g": {"h": null}}),
        );
        assert_eq!(v, json!({"e": 0, "f": false}));
    }

    #[test]
    fn lists_come_bare_or_wrapped() {
        assert_eq!(
            items(json!([{"id": "1"}, {"id": "2", "x": null}]), "k").len(),
            2
        );
        assert_eq!(
            items(json!({"k": [{"id": "1"}]}), "k"),
            vec![json!({"id": "1"})]
        );
        assert!(items(json!({"other": 1}), "k").is_empty());
        let l = list(vec![json!(1)], Some("pass to=... for older".into()));
        assert_eq!(l["count"], 1);
        assert_eq!(l["more"], true);
    }

    #[test]
    fn amounts_are_summed_exactly() {
        assert_eq!(minor(Some(&json!(10.5))), Some(1050));
        assert_eq!(minor(Some(&json!(-0.1))), Some(-10));
        assert_eq!(minor(Some(&json!("1234.56"))), Some(123456));
        assert_eq!(minor(Some(&json!(7))), Some(700));
        assert_eq!(minor(Some(&json!("x"))), None);
        assert_eq!(money(-1050), "-10.50");
        assert_eq!(money(5), "0.05");
    }

    #[test]
    fn card_secrets_are_dropped() {
        let mut v =
            json!({"cards": [{"id": "c", "pan": "4111", "cvv": "123", "last_digits": "1111"}]});
        for_output(&mut v);
        assert_eq!(v, json!({"cards": [{"id": "c", "last_digits": "1111"}]}));
    }
}
