use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use serde::de::DeserializeOwned;
use serde_json::Value;

const GLOBAL_MIN_REQUEST_GAP: Duration = Duration::from_millis(350);

static GLOBAL_THROTTLE: OnceLock<Mutex<Instant>> = OnceLock::new();

#[derive(Debug)]
pub struct RequestError {
    pub message: String,
    pub retryable: bool,
    pub safe_to_retry_create: bool,
}

static API_CLIENT: OnceLock<Client> = OnceLock::new();
static STREAM_CLIENT: OnceLock<Client> = OnceLock::new();

fn shared_client(cell: &OnceLock<Client>, read_timeout: Duration) -> Client {
    cell.get_or_init(|| {
        Client::builder()
            .timeout(read_timeout)
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("failed to build reqwest client")
    })
    .clone()
}

pub fn build_client() -> Client {
    shared_client(&API_CLIENT, Duration::from_secs(60))
}

pub fn stream_client() -> Client {
    shared_client(&STREAM_CLIENT, Duration::from_secs(5 * 60))
}

pub fn parse_json<T: DeserializeOwned>(response: Response) -> Result<T, RequestError> {
    let status = response.status();
    let body = response.text().map_err(|err| RequestError {
        message: format!("Failed to read API response: {err}"),
        retryable: true,
        safe_to_retry_create: false,
    })?;

    if !status.is_success() {
        return Err(status_error(status, &body));
    }

    serde_json::from_str(&body).map_err(|err| RequestError {
        message: format!("Failed to parse API response JSON: {err}"),
        retryable: false,
        safe_to_retry_create: false,
    })
}

pub fn status_error(status: StatusCode, body: &str) -> RequestError {
    let message = parse_api_error(body)
        .unwrap_or_else(|| format!("The API returned HTTP {}.", status.as_u16()));
    let transient = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
    RequestError {
        message,
        retryable: transient && !is_out_of_quota(body),
        safe_to_retry_create: true,
    }
}

fn is_out_of_quota(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    value.get("error").is_some_and(|error| {
        ["code", "type"]
            .iter()
            .any(|field| error.get(field).and_then(Value::as_str) == Some("insufficient_quota"))
    })
}

fn parse_api_error(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body).ok().and_then(|value| {
        value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    })
}

pub fn classify_transport_error(err: reqwest::Error) -> RequestError {
    let retryable = err.is_connect() || err.is_timeout() || err.is_request();
    RequestError {
        message: format!("Request failed: {err}"),
        retryable,
        safe_to_retry_create: err.is_connect(),
    }
}

pub fn throttle() {
    let mutex = GLOBAL_THROTTLE.get_or_init(|| Mutex::new(Instant::now() - GLOBAL_MIN_REQUEST_GAP));
    let mut last = mutex.lock().expect("global throttle lock poisoned");
    let now = Instant::now();
    let earliest = *last + GLOBAL_MIN_REQUEST_GAP;
    if earliest > now {
        thread::sleep(earliest - now);
    }
    *last = Instant::now();
}
