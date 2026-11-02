use diwop_begleitapp::{create_app, web};
use reqwest::header;
use tokio::net::TcpListener;

/// Starts an ephemeral test server running the application router and returns its base URL.
async fn start_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    let app = create_app();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    url
}

/// Verifies that an unauthenticated GET to `/` renders the login card.
#[tokio::test]
async fn test_unauthenticated_index_serves_login_page() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let res = client.get(format!("{}/", base_url)).send().await.unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    let html = res.text().await.unwrap();
    assert!(
        html.contains("Admin-Anmeldung"),
        "HTML should contain login heading"
    );
    assert!(
        html.contains("action=\"/login\""),
        "HTML should contain form targeting /login"
    );
    assert!(
        html.contains("name=\"password\""),
        "HTML should contain password input"
    );
    assert!(
        html.contains("autocomplete=\"current-password\""),
        "HTML should contain autocomplete=current-password"
    );
    assert!(
        !html.contains("Dialogübersetzung ausprobieren"),
        "HTML should not expose dialog translation link when unauthenticated"
    );
    assert!(
        !html.contains("Automatische Sprechererkennung ausprobieren"),
        "HTML should not expose vad-diarization link when unauthenticated"
    );
}

/// Verifies that POST `/login` with an incorrect password returns 401 Unauthorized and an error message.
#[tokio::test]
async fn test_login_failure_invalid_password() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let form_params = [("password", "wrong-password")];
    let res = client
        .post(format!("{}/login", base_url))
        .form(&form_params)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);
    let html = res.text().await.unwrap();
    assert!(
        html.contains("Ungültiges Admin-Passwort."),
        "HTML should display invalid password error message"
    );
}

/// Verifies that POST `/login` with a valid password responds with 303 Redirect and sets the session cookie.
#[tokio::test]
async fn test_login_success_sets_cookie_and_redirects() {
    let base_url = start_server().await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let form_params = [("password", "admin")];
    let res = client
        .post(format!("{}/login", base_url))
        .form(&form_params)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok()),
        Some("/")
    );

    let set_cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|h| h.to_str().ok())
        .expect("Set-Cookie header should be present");

    assert!(
        set_cookie.starts_with("admin_session="),
        "Cookie should start with admin_session="
    );
    assert!(
        set_cookie.contains("HttpOnly"),
        "Cookie must specify HttpOnly"
    );
    assert!(
        set_cookie.contains("SameSite=Lax"),
        "Cookie must specify SameSite=Lax"
    );
    assert!(set_cookie.contains("Path=/"), "Cookie must specify Path=/");
}

/// Verifies that GET `/` with a valid session cookie serves the authenticated dashboard view.
#[tokio::test]
async fn test_authenticated_index_serves_dashboard() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let valid_token = web::generate_session_token("admin");

    let res = client
        .get(format!("{}/", base_url))
        .header(header::COOKIE, format!("admin_session={valid_token}"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let html = res.text().await.unwrap();
    assert!(
        html.contains("Willkommen im Administrationsbereich"),
        "HTML should display the dashboard welcome message"
    );
    assert!(
        html.contains("action=\"/logout\""),
        "HTML should contain logout form"
    );
    assert!(
        html.contains("Dialogübersetzung ausprobieren"),
        "HTML should contain dialog translation link on dashboard"
    );
    assert!(
        html.contains("href=\"/dialog-translation\""),
        "HTML should link to /dialog-translation on dashboard"
    );
    assert!(
        html.contains("Automatische Sprechererkennung ausprobieren"),
        "HTML should contain vad-diarization link on dashboard"
    );
    assert!(
        html.contains("href=\"/vad-diarization\""),
        "HTML should link to /vad-diarization on dashboard"
    );
}

/// Verifies that POST `/logout` clears the session cookie and redirects to `/`.
#[tokio::test]
async fn test_logout_clears_cookie() {
    let base_url = start_server().await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let res = client
        .post(format!("{}/logout", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok()),
        Some("/")
    );

    let set_cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|h| h.to_str().ok())
        .expect("Set-Cookie header should be present");

    assert!(
        set_cookie.contains("Max-Age=0"),
        "Logout should clear cookie with Max-Age=0"
    );
}

/// Verifies that GET `/static/style.css` serves the embedded CSS with proper content-type, ETag, and caching headers.
#[tokio::test]
async fn test_static_style_css() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/static/style.css", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok()),
        Some("text/css; charset=utf-8")
    );
    assert_eq!(
        res.headers()
            .get(header::CACHE_CONTROL)
            .and_then(|h| h.to_str().ok()),
        Some("no-cache")
    );
    assert!(
        res.headers().contains_key(header::ETAG),
        "Response should include ETag header"
    );

    let css = res.text().await.unwrap();
    assert!(
        css.contains("color-scheme: light dark;"),
        "CSS should configure color-scheme"
    );
    assert!(
        css.contains(".login-card"),
        "CSS should style .login-card component"
    );
}

/// Verifies that static assets (both compile-time embedded and on-disk WASM binaries) support
/// ETag validation via `If-None-Match`, returning `304 Not Modified` on cache hits and `200 OK` on mismatches.
#[tokio::test]
async fn test_static_asset_etag_validation_and_not_modified() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    for path in ["/static/style.css", "/static/transcript_alignment.js"] {
        let url = format!("{base_url}{path}");

        // 1. Initial request without If-None-Match -> 200 OK with ETag & no-cache
        let initial_res = client.get(&url).send().await.unwrap();
        assert_eq!(
            initial_res.status(),
            reqwest::StatusCode::OK,
            "Initial request should succeed with 200 OK for {path}"
        );
        assert_eq!(
            initial_res
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|h| h.to_str().ok()),
            Some("no-cache"),
            "Static asset should enforce no-cache revalidation for {path}"
        );
        assert_eq!(
            initial_res
                .headers()
                .get("cross-origin-embedder-policy")
                .and_then(|h| h.to_str().ok()),
            Some("require-corp"),
            "Static asset must include Cross-Origin-Embedder-Policy: require-corp for Chrome Worker contexts ({path})"
        );
        assert_eq!(
            initial_res
                .headers()
                .get("cross-origin-resource-policy")
                .and_then(|h| h.to_str().ok()),
            Some("same-origin"),
            "Static asset must include Cross-Origin-Resource-Policy: same-origin ({path})"
        );

        let etag = initial_res
            .headers()
            .get(header::ETAG)
            .and_then(|h| h.to_str().ok())
            .expect("Response should contain an ETag header")
            .to_string();
        assert!(
            etag.starts_with('"') && etag.ends_with('"'),
            "ETag should be quoted per RFC 7232: {etag}"
        );

        // 2. Subsequent request with matching If-None-Match -> 304 Not Modified with empty body
        let reval_res = client
            .get(&url)
            .header(header::IF_NONE_MATCH, &etag)
            .send()
            .await
            .unwrap();
        assert_eq!(
            reval_res.status(),
            reqwest::StatusCode::NOT_MODIFIED,
            "Revalidation with matching ETag should return 304 Not Modified for {path}"
        );
        assert_eq!(
            reval_res
                .headers()
                .get(header::ETAG)
                .and_then(|h| h.to_str().ok()),
            Some(etag.as_str()),
            "304 response should preserve the ETag header for {path}"
        );
        assert_eq!(
            reval_res
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|h| h.to_str().ok()),
            Some("no-cache"),
            "304 response should include Cache-Control for {path}"
        );
        assert_eq!(
            reval_res
                .headers()
                .get("cross-origin-embedder-policy")
                .and_then(|h| h.to_str().ok()),
            Some("require-corp"),
            "304 response must preserve Cross-Origin-Embedder-Policy: require-corp for {path}"
        );
        let body_bytes = reval_res.bytes().await.unwrap();
        assert!(
            body_bytes.is_empty(),
            "304 Not Modified response body must be empty"
        );

        // 3. Subsequent request with mismatched If-None-Match -> 200 OK with full body
        let mismatch_res = client
            .get(&url)
            .header(header::IF_NONE_MATCH, "\"outdated-etag-hash\"")
            .send()
            .await
            .unwrap();
        assert_eq!(
            mismatch_res.status(),
            reqwest::StatusCode::OK,
            "Revalidation with outdated ETag should return 200 OK for {path}"
        );
        assert_eq!(
            mismatch_res
                .headers()
                .get(header::ETAG)
                .and_then(|h| h.to_str().ok()),
            Some(etag.as_str())
        );
        let mismatch_bytes = mismatch_res.bytes().await.unwrap();
        assert!(
            !mismatch_bytes.is_empty(),
            "200 response body must contain asset payload"
        );
    }
}

/// Verifies that an unauthenticated GET to `/dialog-translation` presents the login form with next parameter.
#[tokio::test]
async fn test_unauthenticated_dialog_translation_serves_login() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/dialog-translation", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let html = res.text().await.unwrap();
    assert!(
        html.contains("Admin-Anmeldung"),
        "HTML should contain login heading"
    );
    assert!(
        html.contains("name=\"next\" value=\"/dialog-translation\""),
        "HTML should contain hidden next field targeting /dialog-translation"
    );
}

/// Verifies that POST `/login` with a valid password and `next` parameter redirects to the specified path.
#[tokio::test]
async fn test_login_success_redirects_to_next() {
    let base_url = start_server().await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let form_params = [("password", "admin"), ("next", "/dialog-translation")];
    let res = client
        .post(format!("{}/login", base_url))
        .form(&form_params)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok()),
        Some("/dialog-translation")
    );
}

/// Verifies that GET `/dialog-translation` with a valid session cookie renders the dialog translation interface.
#[tokio::test]
async fn test_authenticated_dialog_translation_serves_interface() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let valid_token = web::generate_session_token("admin");

    let res = client
        .get(format!("{}/dialog-translation", base_url))
        .header(header::COOKIE, format!("admin_session={valid_token}"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let html = res.text().await.unwrap();
    assert!(
        html.contains("Dialog in Leichter Sprache"),
        "HTML should contain page title"
    );
    assert!(
        html.contains("record-user-btn"),
        "HTML should contain user record button"
    );
    assert!(
        html.contains("record-partner-btn"),
        "HTML should contain partner record button"
    );
    assert!(
        html.contains("NutzerIn"),
        "HTML should display NutzerIn subtitle"
    );
    assert!(
        html.contains("GesprächspartnerIn"),
        "HTML should display GesprächspartnerIn subtitle"
    );
    assert!(
        html.contains("Neuer Dialog"),
        "HTML should contain reset button"
    );
    assert!(
        html.contains("toggle-debug-btn"),
        "HTML should contain toggle-debug-btn"
    );
    assert!(
        html.contains("dialog-header-stats"),
        "HTML should contain header stats container"
    );
    assert!(
        html.contains("stats-input-badge"),
        "HTML should contain input tokens badge"
    );
    assert!(
        html.contains("stats-output-badge"),
        "HTML should contain output tokens badge"
    );
    assert!(
        html.contains("stats-cost-badge"),
        "HTML should contain cost badge"
    );
    assert!(
        html.contains("attachDebugInfo"),
        "HTML should contain attachDebugInfo script logic"
    );
    assert!(
        html.contains("Prompt-Details"),
        "HTML should contain Prompt-Details label"
    );
    assert!(
        html.contains("System-Prompt"),
        "HTML should contain System-Prompt label"
    );
    assert!(
        html.contains("Beispiel"),
        "HTML should contain German Beispiel label for examples"
    );
    assert!(
        html.contains("card-header-badges"),
        "HTML should contain card header badges container"
    );
    assert!(
        html.contains("card-cost-badge"),
        "HTML should contain card cost badge"
    );
    assert!(
        html.contains("card-input-tokens-badge"),
        "HTML should contain card input tokens badge"
    );
    assert!(
        html.contains("buildTranslateMessages"),
        "HTML should build chronological translate messages from updated history"
    );
    assert!(
        html.contains("historyEntry.message = data.message"),
        "HTML should update historyEntry.message when /correct-transcript completes"
    );
    assert!(
        html.contains("role: 'partner_simple'"),
        "HTML should record partner_simple entries in dialogHistory"
    );
    assert!(
        html.contains("Klassifizierung"),
        "HTML should render classification debug disclosure"
    );
    assert!(
        html.contains("debug-classification-details"),
        "HTML should contain debug-classification-details element class"
    );
    assert!(
        html.contains("JSON-Ausgabe"),
        "HTML should contain JSON-Ausgabe sub-block for classification"
    );
}

/// Verifies that an unauthenticated GET to `/vad-diarization` presents the login form with next parameter.
#[tokio::test]
async fn test_unauthenticated_vad_diarization_serves_login() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/vad-diarization", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let html = res.text().await.unwrap();
    assert!(
        html.contains("Admin-Anmeldung"),
        "HTML should contain login heading"
    );
    assert!(
        html.contains("name=\"next\" value=\"/vad-diarization\""),
        "HTML should contain hidden next field targeting /vad-diarization"
    );
}

/// Verifies that authenticated GET `/vad-diarization` serves the diarization UI with COOP/COEP headers,
/// and embedded/allowlisted static scripts enforce CORP headers and reject unknown paths.
#[tokio::test]
async fn test_authenticated_vad_diarization_and_static_assets() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let valid_token = web::generate_session_token("admin");

    let res = client
        .get(format!("{}/vad-diarization", base_url))
        .header(header::COOKIE, format!("admin_session={valid_token}"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    assert_eq!(
        res.headers()
            .get("cross-origin-opener-policy")
            .and_then(|h| h.to_str().ok()),
        Some("same-origin")
    );
    assert_eq!(
        res.headers()
            .get("cross-origin-embedder-policy")
            .and_then(|h| h.to_str().ok()),
        Some("require-corp")
    );

    let html = res.text().await.unwrap();
    assert!(
        html.contains("Automatische Sprechererkennung"),
        "HTML should contain page headline"
    );
    assert!(
        html.contains("record-diarization-btn"),
        "HTML should contain single record button"
    );
    assert!(
        html.contains("DiarizationEngine"),
        "HTML should instantiate WASM DiarizationEngine"
    );
    assert!(
        html.contains("calibration-view"),
        "HTML should contain calibration view container"
    );
    assert!(
        html.contains("Stimme von NutzerIn kalibrieren"),
        "HTML should contain calibration heading"
    );
    assert!(
        html.contains("recalibrate-voice-btn"),
        "HTML should contain recalibrate voice button"
    );
    assert!(
        html.contains("Stimme von NutzerIn neu kalibrieren"),
        "HTML should contain recalibrate voice button text"
    );
    assert!(
        html.contains("start_onboarding"),
        "HTML should invoke start_onboarding for calibration"
    );
    assert!(
        html.contains("finish_onboarding"),
        "HTML should invoke finish_onboarding for calibration"
    );
    assert!(
        html.contains("load_profiles"),
        "HTML should invoke load_profiles on startup"
    );
    assert!(
        html.contains("GesprächspartnerIn"),
        "HTML should format secondary speakers as GesprächspartnerIn"
    );
    assert!(
        html.contains("/static/transcript_alignment.js"),
        "HTML should load the streaming SpeechRecognition transcript alignment module"
    );
    assert!(
        html.contains("dialog-header-stats"),
        "HTML should render cumulative token and cost badges in the header"
    );
    assert!(
        html.contains("toggle-debug-btn"),
        "HTML should contain toggle-debug-btn"
    );
    assert!(
        html.contains("/correct-transcript"),
        "HTML should integrate POST /correct-transcript for diarized turns"
    );
    assert!(
        html.contains("/translate"),
        "HTML should integrate POST /translate for partner turns"
    );
    assert!(
        html.contains("buildTranslateHistoryMessages"),
        "HTML should build /translate conversation history with corrected transcripts and partner_simple entries"
    );
    assert!(
        html.contains("card-speaker-leichte"),
        "HTML should render Leichte Sprache translation cards with speaker accent styling"
    );
    assert!(
        html.contains("card-likelihood-badge"),
        "HTML should render user likelihood indicator badge at the top of the card"
    );
    assert!(
        html.contains("Nutzer-Wahrscheinlichkeit"),
        "HTML should format user likelihood badge text in German"
    );
    assert!(
        html.contains("pyannote_segmentation_3_0.onnx"),
        "HTML should load pyannote/segmentation-3.0 ONNX model (PRD-3)"
    );
    assert!(
        html.contains("wespeaker_voxblink2_samresnet100_LM.int8.onnx"),
        "HTML should load WeSpeaker SimAM-ResNet100 INT8 ONNX model (PRD-3)"
    );
    assert!(
        html.contains("Können Sie mir das bitte kurz in einfachen Worten erklären?"),
        "HTML should include conversational calibration quote with greeting and question (PRD-3)"
    );

    // Embedded JS assets should return 200 OK with CORP header
    for path in [
        "/static/ort_bridge.js",
        "/static/vad_worklet.js",
        "/static/transcript_alignment.js",
    ] {
        let asset_res = client
            .get(format!("{}{}", base_url, path))
            .send()
            .await
            .unwrap();
        assert_eq!(asset_res.status(), reqwest::StatusCode::OK);
        assert_eq!(
            asset_res
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok()),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(
            asset_res
                .headers()
                .get("cross-origin-resource-policy")
                .and_then(|h| h.to_str().ok()),
            Some("same-origin")
        );
        if path == "/static/transcript_alignment.js" {
            let js_body = asset_res.text().await.unwrap();
            assert!(
                js_body.contains("CORRECT_TRANSCRIPT_SILENCE_SEC"),
                "transcript_alignment.js should export 1.0s correction silence constant"
            );
            assert!(
                js_body.contains("TRANSLATE_LOCK_SILENCE_SEC"),
                "transcript_alignment.js should export 2.0s translation lock silence constant"
            );
            assert!(
                js_body.contains("buildTranslateHistoryMessages"),
                "transcript_alignment.js should export /translate history builder"
            );
            assert!(
                js_body.contains("partner_simple"),
                "transcript_alignment.js should include partner_simple entries in /translate history"
            );
            assert!(
                js_body.contains("getUserLikelihoodForSpan"),
                "transcript_alignment.js should compute duration-weighted user likelihood for turns"
            );
            assert!(
                js_body.contains("userLikelihood"),
                "transcript_alignment.js should track userLikelihood on segments and turns"
            );
            assert!(
                js_body.contains("clusterRelabels"),
                "transcript_alignment.js should process retroactive clusterRelabels (PRD-3)"
            );
        }
    }

    // Disallowed filenames must return 404 Not Found
    let bad_res = client
        .get(format!("{}/static/models/unknown_model.onnx", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_res.status(), reqwest::StatusCode::NOT_FOUND);
}

/// Verifies that an unauthenticated GET to `/analytics` redirects or serves the login card with next=/analytics.
#[tokio::test]
async fn test_unauthenticated_analytics_serves_login_page() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/analytics", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    let html = res.text().await.unwrap();
    assert!(
        html.contains("Admin-Anmeldung"),
        "HTML should contain login heading"
    );
    assert!(
        html.contains("value=\"/analytics\""),
        "HTML should contain next parameter pointing to /analytics"
    );
}

/// Verifies that GET `/analytics` with a valid session cookie serves the Nutzungsanalyse page.
#[tokio::test]
async fn test_authenticated_analytics_page() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let valid_token = web::generate_session_token("admin");

    let res = client
        .get(format!("{}/analytics", base_url))
        .header(header::COOKIE, format!("admin_session={valid_token}"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let html = res.text().await.unwrap();
    assert!(
        html.contains("Nutzungsanalyse"),
        "HTML should contain 'Nutzungsanalyse' title"
    );
    assert!(
        html.contains("kpi-total-requests"),
        "HTML should contain KPI card for total requests"
    );
    assert!(
        html.contains("kpi-error-rate"),
        "HTML should contain KPI card for error rate"
    );
    assert!(
        html.contains("kpi-avg-latency"),
        "HTML should contain KPI card for avg latency"
    );
    assert!(
        html.contains("kpi-total-tokens"),
        "HTML should contain KPI card for total tokens"
    );
    assert!(
        html.contains("chart-requests-correct-transcript"),
        "HTML should contain canvas for correct-transcript requests chart"
    );
    assert!(
        html.contains("chart-requests-translate"),
        "HTML should contain canvas for translate requests chart"
    );
    assert!(
        html.contains("API Anfragen: correct-transcript"),
        "HTML should contain title for correct-transcript API requests chart"
    );
    assert!(
        html.contains("API Anfragen: translate"),
        "HTML should contain title for translate API requests chart"
    );
    assert!(
        html.contains("chart-latencies"),
        "HTML should contain canvas for latencies chart"
    );
    assert!(
        html.contains("chart-tokens"),
        "HTML should contain canvas for tokens chart"
    );
    assert!(
        html.contains("chart-topics"),
        "HTML should contain canvas for topics chart"
    );
    assert!(
        html.contains("chart-categories"),
        "HTML should contain canvas for categories chart"
    );
    assert!(
        html.contains("chart-types"),
        "HTML should contain canvas for types chart"
    );
    assert!(
        html.contains("chart-history"),
        "HTML should contain canvas for history distribution chart"
    );
    assert!(
        html.contains("/static/chart.js"),
        "HTML should include local Chart.js bundle"
    );
}

/// Verifies that GET `/api/admin/analytics` without authentication returns 401 Unauthorized.
#[tokio::test]
async fn test_analytics_api_unauthorized() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/api/admin/analytics", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);
}

/// Verifies that GET `/api/admin/analytics` with admin authentication returns valid JSON report.
#[tokio::test]
async fn test_analytics_api_authorized() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let valid_token = web::generate_session_token("admin");

    // Record some sample metrics
    diwop_begleitapp::metrics::record_http_request("/translate", 200, 0.125);
    diwop_begleitapp::metrics::record_token_usage("translate", 80, 20, 30);
    diwop_begleitapp::metrics::record_token_usage("classify", 40, 5, 10);
    diwop_begleitapp::metrics::record_translate_history_length(3);
    diwop_begleitapp::metrics::record_translate_classification("medical", "translation", "warning");

    let res = client
        .get(format!("{}/api/admin/analytics?range=24h", base_url))
        .header(header::COOKIE, format!("admin_session={valid_token}"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let json: serde_json::Value = res.json().await.unwrap();

    assert_eq!(json["time_range"], "24h");
    assert!(json["summary"]["total_requests"].as_u64().unwrap() >= 1);
    assert!(json["summary"]["total_tokens"].as_u64().unwrap() >= 185);

    let endpoints = json["requests_by_endpoint"].as_array().unwrap();
    assert!(endpoints.iter().any(|e| e["endpoint"] == "/translate"));

    let over_time = json["requests_over_time"].as_array().unwrap();
    assert!(over_time
        .iter()
        .any(|e| e["endpoint"] == "/correct-transcript"));
    assert!(over_time.iter().any(|e| e["endpoint"] == "/translate"));

    let tokens_by_prompt = json["token_usage_by_prompt"].as_array().unwrap();
    assert!(tokens_by_prompt
        .iter()
        .any(|p| p["prompt"] == "correct-transcript"));
    assert!(tokens_by_prompt.iter().any(|p| p["prompt"] == "translate"));
    assert!(tokens_by_prompt.iter().any(|p| p["prompt"] == "classify"));

    let topics = json["translation_by_topic"].as_array().unwrap();
    assert!(topics.iter().any(|t| t["topic"] == "medical"));

    let categories = json["translation_by_category"].as_array().unwrap();
    assert!(categories.iter().any(|c| c["category"] == "warning"));

    let history = json["history_length_distribution"].as_array().unwrap();
    assert!(history.iter().any(|h| h["bucket"] == "3-5"));
}

/// Verifies that GET `/static/chart.js` serves the vendored Chart.js bundle with ETag and 304 handling.
#[tokio::test]
async fn test_static_chart_js() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/static/chart.js", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok()),
        Some("text/javascript; charset=utf-8")
    );

    let etag = res
        .headers()
        .get(header::ETAG)
        .and_then(|h| h.to_str().ok())
        .expect("ETag header should be present")
        .to_string();

    // Conditional GET with If-None-Match
    let not_modified_res = client
        .get(format!("{}/static/chart.js", base_url))
        .header(header::IF_NONE_MATCH, etag)
        .send()
        .await
        .unwrap();

    assert_eq!(not_modified_res.status(), reqwest::StatusCode::NOT_MODIFIED);
}

/// Verifies that the dashboard contains a link to `/analytics`.
#[tokio::test]
async fn test_dashboard_contains_analytics_link() {
    let base_url = start_server().await;
    let client = reqwest::Client::new();

    let valid_token = web::generate_session_token("admin");

    let res = client
        .get(format!("{}/", base_url))
        .header(header::COOKIE, format!("admin_session={valid_token}"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let html = res.text().await.unwrap();
    assert!(
        html.contains("href=\"/analytics\""),
        "Dashboard should link to /analytics"
    );
    assert!(
        html.contains("Nutzungsanalyse ansehen"),
        "Dashboard should contain link text 'Nutzungsanalyse ansehen'"
    );
}
