use serde_json::Value;

pub fn language(preferences: &[String]) -> &'static str {
    for preference in preferences {
        let tag = preference.to_lowercase().replace('_', "-");
        if tag == "en" || tag.starts_with("en-") {
            return "en";
        }
        if ["zh", "zh-cn", "zh-sg"].contains(&tag.as_str()) || tag.starts_with("zh-hans") {
            return "zh-Hans";
        }
    }
    "en"
}
pub struct Localizer {
    pub language: String,
    en: Value,
    zh: Value,
}
impl Localizer {
    pub fn new() -> Self {
        Self {
            language: "en".into(),
            en: serde_json::from_str(include_str!("../../locales/en.json")).unwrap(),
            zh: serde_json::from_str(include_str!("../../locales/zh-Hans.json")).unwrap(),
        }
    }
    pub fn text(&self, key: &str) -> String {
        self.format(key, &[])
    }
    pub fn format(&self, key: &str, values: &[(&str, String)]) -> String {
        let catalog = if self.language == "zh-Hans" {
            &self.zh
        } else {
            &self.en
        };
        let value = catalog.get(key).or_else(|| self.en.get(key));
        let count = values
            .iter()
            .find(|(k, _)| *k == "count")
            .and_then(|(_, v)| v.parse::<f64>().ok());
        let category = if self.language == "en" && count == Some(1.) {
            "one"
        } else {
            "other"
        };
        let message = value
            .and_then(|v| {
                v.as_str().or_else(|| {
                    v.get(category)
                        .or_else(|| v.get("other"))
                        .and_then(Value::as_str)
                })
            })
            .unwrap_or(key);
        // Single pass: placeholders inside app names stay literal.
        let mut out = String::new();
        let mut rest = message;
        while let Some(i) = rest.find('{') {
            out.push_str(&rest[..i]);
            rest = &rest[i..];
            if let Some(j) = rest.find('}') {
                let name = &rest[1..j];
                if let Some((_, v)) = values.iter().find(|(k, _)| *k == name) {
                    out.push_str(v)
                } else {
                    out.push_str(&rest[..=j])
                }
                rest = &rest[j + 1..];
            } else {
                break;
            }
        }
        out.push_str(rest);
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn language_priority() {
        assert_eq!(language(&["zh-Hant".into(), "en".into()]), "en");
        assert_eq!(language(&["zh_CN".into()]), "zh-Hans");
        assert_eq!(language(&["en-US".into(), "zh-Hans".into()]), "en");
    }
    #[test]
    fn plural_and_literal_values() {
        let mut l = Localizer::new();
        assert_eq!(
            l.format("{count} projects", &[("count", "1".into())]),
            "1 project"
        );
        l.language = "zh-Hans".into();
        assert_eq!(
            l.format("Quit {name}", &[("name", "猫{count}".into())]),
            "退出 猫{count}"
        );
        assert_eq!(l.text("future key"), "future key");
    }
}
