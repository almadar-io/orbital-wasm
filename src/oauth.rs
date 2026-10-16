//! The pure half of `oauth-authorize` for JavaScript hosts: parse the client and options, then build the
//! authorization URL from the metadata and two host-drawn random values. The host does the asks in between
//! (client id, metadata, random, pending grant); every rule is `orbital_core::runtime::oauth_flow`.

use orbital_core::evaluator::Value;
use orbital_core::oauth::client::OAuthServerDecl;
use orbital_core::runtime::oauth_flow::{build_authorization, declared_metadata, plan_authorize, AuthorizePlan, FlowFailure};
use serde_json::{json, Map, Value as Json};

fn parse<T: serde::de::DeserializeOwned>(json: &str, what: &str) -> Result<T, String> {
    serde_json::from_str(json).map_err(|e| format!("parse {what}: {e}"))
}

fn out(value: &Json) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| format!("serialize: {e}"))
}

/// `{ plan, clientId: { name, scope }, issuer: string | null, metadata: object | null }` for the client and options
/// of an `oauth-authorize`; the error text is the effect's failure.
pub fn oauth_authorize_plan(client_json: &str, options_json: &str) -> Result<String, String> {
    let client: Value = parse(client_json, "client_json")?;
    let options: Value = parse(options_json, "options_json")?;
    let plan = plan_authorize(&client, &options)?;
    let (issuer, metadata) = match &plan.client.server {
        OAuthServerDecl::Issuer(issuer) => (Json::String(issuer.clone()), Json::Null),
        endpoints => (Json::Null, declared_metadata(endpoints).map_or(Json::Null, Json::Object)),
    };
    out(&json!({
        "plan": plan,
        "mode": plan.client.mode,
        "clientId": plan.client.client_id,
        "issuer": issuer,
        "metadata": metadata,
    }))
}

/// The mock backend's `{ authUrl, state, grant }` from a plan and the drawn state bytes (standard base64).
pub fn oauth_authorize_mock_build(plan_json: &str, state_base64: &str) -> Result<String, String> {
    let plan: AuthorizePlan = parse(plan_json, "plan_json")?;
    let state = orbital_core::codec::base64_decode(state_base64).map_err(|e| format!("state is not base64: {e}"))?;
    let (auth_url, state, grant) = orbital_core::runtime::oauth_flow::build_mock_authorization(&plan, &state);
    out(&json!({ "authUrl": auth_url, "state": state, "grant": grant }))
}

/// The failure payload `{ error, code, reason }` of one closed reason: the fixed safe message the kernel uses.
pub fn oauth_failure(reason: &str) -> Result<String, String> {
    let reason: orbital_core::oauth::ask::OAuthFailureReason =
        serde_json::from_value(Json::String(reason.to_string())).map_err(|_| format!("unknown oauth failure reason `{reason}`"))?;
    out(&json!({ "error": reason.message(), "code": reason.code(), "reason": reason.as_str() }))
}

/// `{ ok: { authUrl, state, grant } }`, `{ failure: { reason, code, message } }` or `{ refused: string }`.
pub fn oauth_authorize_build(
    plan_json: &str,
    client_id: &str,
    metadata_json: &str,
    state_base64: &str,
    verifier_base64: &str,
) -> Result<String, String> {
    let plan: AuthorizePlan = parse(plan_json, "plan_json")?;
    let metadata: Map<String, Json> = parse(metadata_json, "metadata_json")?;
    let decode = |text: &str, what: &str| orbital_core::codec::base64_decode(text).map_err(|e| format!("{what} is not base64: {e}"));
    let built = build_authorization(&plan, client_id, &metadata, decode(state_base64, "state")?, decode(verifier_base64, "verifier")?);
    match built {
        Ok((auth_url, state, grant)) => out(&json!({ "ok": { "authUrl": auth_url, "state": state, "grant": grant } })),
        Err(FlowFailure::Reason(reason)) => out(&json!({ "failure": { "reason": reason.as_str(), "code": reason.code(), "message": reason.message() } })),
        Err(FlowFailure::Refused(refusal)) => out(&json!({ "refused": refusal.message() })),
        Err(FlowFailure::Transport(_)) => Err("authorization building never sends a request".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Json {
        json!({"tag":"github","mode":"real","endpoints":{"authorization":"https://github.com/login/oauth/authorize","token":"https://github.com/login/oauth/access_token"},"clientId":"ID","clientSecret":"SEC","clientAuth":"post"})
    }

    #[test]
    fn a_plan_names_the_secret_slot_and_static_metadata_and_a_build_keeps_the_verifier_in_the_grant_only() {
        let options = json!({"redirectUri":"https://app/cb","scope":["repo"],"subject":"u1"});
        let planned: Json = serde_json::from_str(&oauth_authorize_plan(&client().to_string(), &options.to_string()).unwrap()).unwrap();
        assert_eq!(planned["clientId"]["name"], "ID");
        assert!(planned["issuer"].is_null());
        assert_eq!(planned["metadata"]["token_endpoint"], "https://github.com/login/oauth/access_token");
        let state = orbital_core::codec::base64_encode(&[1u8; 32]);
        let verifier = orbital_core::codec::base64_encode(&[2u8; 32]);
        let built: Json = serde_json::from_str(
            &oauth_authorize_build(&planned["plan"].to_string(), "client-id", &planned["metadata"].to_string(), &state, &verifier).unwrap(),
        )
        .unwrap();
        let url = built["ok"]["authUrl"].as_str().unwrap();
        assert!(url.starts_with("https://github.com/login/oauth/authorize?") && url.contains("code_challenge="), "{url}");
        assert!(!url.contains(built["ok"]["grant"]["verifier"].as_str().unwrap()));
        assert_eq!(built["ok"]["grant"]["subject"], "u1");
    }

    #[test]
    fn every_closed_reason_has_its_safe_failure_payload_and_an_unknown_one_is_an_error() {
        use orbital_core::oauth::ask::OAuthFailureReason;
        for reason in OAuthFailureReason::ALL {
            let payload: Json = serde_json::from_str(&oauth_failure(reason.as_str()).unwrap()).unwrap();
            assert_eq!(payload, json!({ "error": reason.message(), "code": reason.code(), "reason": reason.as_str() }));
        }
        assert!(oauth_failure("vendor-said-no").is_err());
    }

    #[test]
    fn bad_options_fail_with_the_effects_message_and_a_mock_client_plans_and_builds_with_no_credential() {
        let bad = oauth_authorize_plan(&client().to_string(), &json!({"redirectUri":"u","state":"mine"}).to_string()).unwrap_err();
        assert!(bad.contains("unknown option `state`"), "{bad}");
        assert!(oauth_authorize_plan(&client().to_string(), &json!({"scope":["a"]}).to_string()).is_err());
        let mut mock = client();
        mock["mode"] = json!("mock");
        let planned: Json = serde_json::from_str(&oauth_authorize_plan(&mock.to_string(), &json!({"redirectUri":"https://app/cb","scope":["a","b"],"subject":"u1"}).to_string()).unwrap()).unwrap();
        assert_eq!(planned["mode"], "mock");
        let state = orbital_core::codec::base64_encode(&[0xab; 16]);
        let built: Json = serde_json::from_str(&oauth_authorize_mock_build(&planned["plan"].to_string(), &state).unwrap()).unwrap();
        assert_eq!(built["state"], "ab".repeat(16));
        assert_eq!(
            built["authUrl"],
            format!("https://github.com/login/oauth/authorize?response_type=code&scope=a+b&redirect_uri=https%3A%2F%2Fapp%2Fcb&state={}&client_id=mock-client-id", "ab".repeat(16))
        );
        assert_eq!(built["grant"]["scopes"], json!(["a", "b"]));
        assert_eq!(built["grant"]["subject"], "u1");
    }
}
