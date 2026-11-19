use pro_gui::models::AppSettings;

#[test]
fn settings_debug_output_hides_the_api_keys() {
    let settings = AppSettings {
        api_key: "openai-secret".to_owned(),
        gemini_api_key: "gemini-secret".to_owned(),
        ..AppSettings::default()
    };

    let printed = format!("{settings:?}");

    assert!(!printed.contains("secret"), "{printed}");
    assert!(printed.contains("anthropic_api_key"), "{printed}");
    assert!(printed.contains("font_size: 14.0"), "{printed}");
}
