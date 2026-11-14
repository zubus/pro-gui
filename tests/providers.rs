use pro_gui::providers::*;

#[test]
fn each_models_prefix_routes_it_to_its_api() {
    assert_eq!(Provider::of("gemini-4-carbon"), Provider::Gemini);
    assert_eq!(Provider::of("gemini-4-argon"), Provider::Gemini);
    assert_eq!(Provider::of("claude-fable-5-2"), Provider::Anthropic);
    assert_eq!(
        Provider::of("anthropic.claude-fable-5-2"),
        Provider::Anthropic
    );
    assert_eq!(Provider::of("gpt-6-astra-pro"), Provider::OpenAi);
    assert_eq!(Provider::of("some-custom-id"), Provider::OpenAi);
}

#[test]
fn each_model_gets_its_own_ladder_and_limit() {
    assert_eq!(max_output_tokens("gemini-4-carbon"), MAX_OUTPUT_TOKENS);
    assert_eq!(effort_options("gemini-4-argon"), &["low", "medium", "high"]);
    assert_eq!(default_reasoning_effort("gemini-4-argon"), "high");
    assert_eq!(default_reasoning_effort("gpt-6-astra-pro"), "max");
    assert_eq!(
        effort_options("claude-fable-5-2"),
        &["low", "medium", "high", "xhigh", "max"]
    );
    assert_eq!(default_reasoning_effort("claude-fable-5-2"), "max");
    assert_eq!(default_reasoning_effort("gpt-5.4-pro"), "max");

    assert_eq!(max_output_tokens("gemini-4-argon"), MAX_OUTPUT_TOKENS);
    assert_eq!(
        max_output_tokens("gpt-6-astra-pro"),
        openai::MAX_OUTPUT_TOKENS
    );
    assert_eq!(model_display_name("gemini-4-carbon"), "Gemini 4 Carbon");
    assert_eq!(max_output_tokens("claude-fable-5-2"), 128_000);
    assert_eq!(clamp_output_tokens("gpt-6-astra-pro", 262_144), 128_000);
    assert_eq!(clamp_output_tokens("gemini-4-argon", 262_144), 262_144);
    assert_eq!(clamp_output_tokens("claude-fable-5-2", 262_144), 128_000);
}

#[test]
fn priority_maps_onto_each_apis_tier_names() {
    assert_eq!(request_service_tier("gemini-4-argon", true), "priority");
    assert_eq!(request_service_tier("gemini-4-argon", false), "standard");
    assert_eq!(request_service_tier("gpt-6-astra-pro", false), "default");
    assert_eq!(
        request_service_tier("claude-fable-5-2", true),
        "standard_only"
    );
    assert!(!supports_priority("claude-fable-5-2"));
    assert!(supports_priority("gpt-6-astra-pro"));
    assert_eq!(
        request_service_tier("claude-fable-5-2", false),
        "standard_only"
    );
    assert_eq!(model_display_name("gemini-4-argon"), "Gemini 4 Argon");
    assert_eq!(model_display_name("gpt-6-astra-pro"), "GPT-6 Astra Pro");
    assert_eq!(model_display_name("claude-fable-5-2"), "Claude Fable 5.2");
}

#[test]
fn anthropic_is_stateless_while_the_other_apis_store_runs() {
    assert!(!use_background_mode("claude-fable-5-2"));
    assert!(!store_responses("claude-fable-5-2"));
    assert!(use_background_mode("gpt-6-astra-pro"));
    assert!(use_background_mode("gemini-4-argon"));
}
