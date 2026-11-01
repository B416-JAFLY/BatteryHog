use serde_json::Value;
pub fn num(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite())
}
pub fn n(value: &Value) -> f64 {
    num(value).unwrap_or(0.)
}
pub fn text(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        if value.is_number() {
            value.to_string()
        } else {
            "—".into()
        }
    })
}
pub fn list(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}
pub fn mem(value: &Value) -> String {
    match num(value) {
        Some(v) if v >= 1024. => format!("{:.1} GB", v / 1024.),
        Some(v) => format!("{v:.0} MB"),
        None => "—".into(),
    }
}
pub fn metric(value: &Value, unit: &str) -> String {
    num(value)
        .map(|n| format!("{n:.1}{unit}"))
        .unwrap_or_else(|| "—".into())
}
/// Keep null buckets as gaps and index all buckets against the complete time domain.
pub fn history_points(history: &Value) -> Vec<Option<(f32, bool)>> {
    list(&history["bins"])
        .iter()
        .map(|b| {
            num(&b["v"])
                .or_else(|| num(&b["hi"]))
                .map(|n| (n.clamp(0., 100.) as f32, b["ac"].as_bool().unwrap_or(false)))
        })
        .collect()
}
pub fn sorted_processes(stats: &Value, sort: &str) -> Vec<Value> {
    let mut p = list(&stats["processes"]);
    p.sort_by(|a, b| n(&b[sort]).total_cmp(&n(&a[sort])));
    p
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn sparse_history_preserves_positions() {
        let h = json!({"bins":[null,{"v":110,"ac":true},null,{"hi":30}]});
        let p = history_points(&h);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], None);
        assert_eq!(p[1], Some((100., true)));
        assert_eq!(p[3], Some((30., false)));
    }
    #[test]
    fn missing_metrics_and_sort() {
        assert_eq!(metric(&Value::Null, " W"), "—");
        let p = sorted_processes(
            &json!({"processes":[{"name":"A","cpu":1},{"name":"B","cpu":90}]}),
            "cpu",
        );
        assert_eq!(p[0]["name"], "B");
    }
}
