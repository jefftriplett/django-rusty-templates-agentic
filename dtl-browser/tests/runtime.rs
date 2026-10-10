use dtl_browser::Engine;
use serde_json::Value;

#[test]
fn shared_parity_cases() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("parity.json")).unwrap();
    for case in cases {
        let mut engine = Engine::new();
        let name = case["name"].as_str().unwrap();
        if let Some(templates) = case["templates"].as_object() {
            for (name, source) in templates {
                engine.add_template(name, source.as_str().unwrap()).unwrap();
            }
        }
        engine
            .add_template(name, case["source"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            engine.render(name, &case["context"].to_string()).unwrap(),
            case["expected"].as_str().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn rejects_unsupported_or_malformed_syntax() {
    for source in [
        "{% extends 'base' %}",
        "{% load i18n %}",
        "{{ name|unknown }}",
        "{{ name|default: }}",
        "{{ name|upper:1 }}",
        "{% if True %}unclosed",
        "{% else %}",
        "{% for a,b in rows %}{% endfor %}",
        "{% include 'card' only %}",
        "{% if %}{% endif %}",
    ] {
        assert!(
            Engine::new().add_template("bad", source).is_err(),
            "{source}"
        );
    }
}

#[test]
fn errors_leave_cached_engine_usable() {
    let mut engine = Engine::new();
    engine
        .add_template("cycle", "{% include 'cycle' %}")
        .unwrap();
    engine.add_template("good", "{{ name }}").unwrap();
    assert!(engine.render("cycle", "{}").unwrap_err().contains("depth"));
    assert!(engine.render("absent", "{}").is_err());
    assert!(engine.render("good", "[]").is_err());
    assert!(engine.render("good", "not json").is_err());
    assert!(engine.add_template("good", "{{ name|unknown }}").is_err());
    assert_eq!(
        engine.render("good", r#"{"name":"still works"}"#).unwrap(),
        "still works"
    );
}

#[test]
fn output_and_work_limits() {
    let mut engine = Engine::new();
    engine
        .add_template("large", "{% for item in items %}{{ text }}{% endfor %}")
        .unwrap();
    let context = serde_json::json!({"items":[1,2,3],"text":"x".repeat(500_000)});
    assert!(
        engine
            .render("large", &context.to_string())
            .unwrap_err()
            .contains("output")
    );
    engine
        .add_template(
            "busy",
            "{% for a in items %}{% for b in items %}{% endfor %}{% endfor %}",
        )
        .unwrap();
    let context = serde_json::json!({"items":vec![0;400]});
    assert!(
        engine
            .render("busy", &context.to_string())
            .unwrap_err()
            .contains("step")
    );
}
