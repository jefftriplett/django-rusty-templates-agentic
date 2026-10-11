use dtl_browser::Engine;
use serde_json::Value;

#[test]
fn shared_parity_cases() {
    let mut cases: Vec<Value> = serde_json::from_str(include_str!("parity.json")).unwrap();
    cases.push(serde_json::from_str(include_str!("../demo/catalog.json")).unwrap());
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
        "{% extends 'base' %}{% extends 'other' %}",
        "{% load i18n %}",
        "{{ name|unknown }}",
        "{{ name|default: }}",
        "{{ name|upper:1 }}",
        "{% if True %}unclosed",
        "{% else %}",
        "{% for a,b in rows %}{% endfor %}",
        "{% include 'card' only %}",
        "{% if %}{% endif %}",
        "{% block a %}{% endblock b %}",
        "{% block a %}{% endblock %}{% block a %}{% endblock %}",
        "{% if True %}{% extends 'base' %}{% endif %}",
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

#[test]
fn inheritance_errors_and_recovery() {
    let mut engine = Engine::new();
    engine.add_template("a", "{% extends 'b' %}").unwrap();
    engine.add_template("b", "{% extends 'a' %}").unwrap();
    assert!(
        engine
            .render("a", "{}")
            .unwrap_err()
            .contains("Inheritance cycle")
    );
    engine
        .add_template("missing", "{% extends 'absent' %}")
        .unwrap();
    assert!(
        engine
            .render("missing", "{}")
            .unwrap_err()
            .contains("Template not found")
    );
    engine
        .add_template(
            "base_super",
            "{% block body %}{{ block.super }}{% endblock %}",
        )
        .unwrap();
    assert!(
        engine
            .render("base_super", "{}")
            .unwrap_err()
            .contains("inheritance context")
    );
    engine
        .add_template("good", "{% block body %}good{% endblock %}")
        .unwrap();
    assert_eq!(engine.render("good", "{}").unwrap(), "good");
}

#[test]
fn parent_selection_and_replacement_are_live() {
    let mut engine = Engine::new();
    engine
        .add_template("base", "A:{% block body %}base{% endblock %}")
        .unwrap();
    engine
        .add_template("other", "B:{% block body %}other{% endblock %}")
        .unwrap();
    engine
        .add_template(
            "child",
            "{% extends parent %}{% block body %}{{ block.super }}+child{% endblock %}",
        )
        .unwrap();
    assert_eq!(
        engine.render("child", r#"{"parent":"base"}"#).unwrap(),
        "A:base+child"
    );
    assert_eq!(
        engine.render("child", r#"{"parent":"other"}"#).unwrap(),
        "B:other+child"
    );
    engine
        .add_template("base", "C:{% block body %}changed{% endblock %}")
        .unwrap();
    assert_eq!(
        engine.render("child", r#"{"parent":"base"}"#).unwrap(),
        "C:changed+child"
    );
}
