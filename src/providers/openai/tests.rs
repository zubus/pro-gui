use super::*;

#[test]
fn pro_options_map_to_their_base_model_in_pro_mode() {
    assert_eq!(api_model_id(ASTRA_PRO_MODEL), ASTRA_MODEL);
    assert_eq!(reasoning_mode(ASTRA_PRO_MODEL), Some("pro"));

    assert_eq!(api_model_id(SOL_PRO_MODEL), SOL_MODEL);
    assert_eq!(reasoning_mode(SOL_PRO_MODEL), Some("pro"));
}

#[test]
fn base_models_are_sent_unchanged_without_a_mode() {
    for model in [ASTRA_MODEL, SOL_MODEL, GPT_55_PRO_MODEL, "some-custom-id"] {
        assert_eq!(api_model_id(model), model);
        assert_eq!(reasoning_mode(model), None, "{model} should use standard");
    }
}

#[test]
fn every_model_defaults_to_max_effort() {
    for model in [
        ASTRA_PRO_MODEL,
        ASTRA_MODEL,
        SOL_PRO_MODEL,
        SOL_MODEL,
        GPT_55_PRO_MODEL,
    ] {
        assert_eq!(default_reasoning_effort(model), "max", "{model}");
    }
    assert!(!REASONING_EFFORT_OPTIONS.contains(&"none"));
}

#[test]
fn astra_pro_survives_being_served_as_plain_astra() {
    assert_eq!(
        stored_model_id(ASTRA_PRO_MODEL, Some(ASTRA_MODEL.to_owned())),
        Some(ASTRA_PRO_MODEL.to_owned())
    );
    assert_eq!(
        stored_model_id("gpt-6", Some(ASTRA_MODEL.to_owned())),
        Some(ASTRA_MODEL.to_owned())
    );
}

#[test]
fn a_dated_snapshot_keeps_the_pro_model_that_was_asked_for() {
    assert_eq!(
        stored_model_id(ASTRA_PRO_MODEL, Some(format!("{ASTRA_MODEL}-2026-09-30"))),
        Some(ASTRA_PRO_MODEL.to_owned())
    );
    assert_eq!(
        stored_model_id(ASTRA_MODEL, Some(format!("{ASTRA_MODEL}-mini"))),
        Some(format!("{ASTRA_MODEL}-mini"))
    );
}

#[test]
fn new_installs_default_to_astra_pro() {
    assert_eq!(normalize_model("  "), ASTRA_PRO_MODEL);
    assert_eq!(model_display_name(ASTRA_PRO_MODEL), "GPT-6 Astra Pro");
    assert_eq!(MODEL_OPTIONS[0].id, ASTRA_PRO_MODEL);
}

#[test]
fn openai_request_carries_pdf_as_input_file_and_images_as_input_image() {
    let body = request_body(&RequestSpec {
        model: ASTRA_PRO_MODEL.to_owned(),
        prompt: "Summarize.".to_owned(),
        images: vec![
            "data:image/png;base64,iVBORw0K".to_owned(),
            "data:application/pdf;base64,JVBERi0xLjQ=".to_owned(),
        ],
        instructions: String::new(),
        max_output_tokens: 4096,
        reasoning_effort: "max".to_owned(),
        service_tier: "default".to_owned(),
        history: Vec::new(),
        previous_response_id: None,
    });
    assert_eq!(
        body["input"],
        json!([{
            "role": "user",
            "content": [
                { "type": "input_text", "text": "Summarize." },
                { "type": "input_image", "image_url": "data:image/png;base64,iVBORw0K" },
                {
                    "type": "input_file",
                    "filename": "document.pdf",
                    "file_data": "data:application/pdf;base64,JVBERi0xLjQ="
                }
            ]
        }])
    );
}

#[test]
fn polling_slows_down_as_a_run_ages() {
    assert_eq!(poll_interval(Duration::ZERO), POLL_INTERVAL);
    assert_eq!(
        poll_interval(Duration::from_secs(180)),
        Duration::from_secs(5)
    );
    assert_eq!(
        poll_interval(Duration::from_secs(4 * 3600)),
        MAX_POLL_INTERVAL
    );
}
