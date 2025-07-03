use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};
use serde_json::{json, Value};
use reqwest::{header, Method};
use sha2::Sha256;
use hmac::{Hmac, Mac};

pub trait Manager {
    fn auth(
        &self,
        req_params: &str,
        recv_window: u64,
        timestamp: u64,
    ) -> Result<String, &'static str>;
    fn prepare_payload(&self, method: &str, parameters: HashMap<String, String>) -> String;
    fn generate_timestamp(&self) -> u64;
    async fn submit_request(
        &self,
        method: Method,
        path: &str,
        query: HashMap<String, String>,
        auth: bool,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>>;
}

pub struct HttpManager {
    api_key: String,
    api_secret: String,
    base_url: String,
    recv_window: u64,
    ignore_codes: Vec<u64>,
    max_retries: u64,
    client: reqwest::Client,
}

impl HttpManager {
    pub fn new(api_key: String, api_secret: String, testnet: bool) -> Self {
        let sub_domain = if testnet { "api-testnet" } else { "api" };
        let domain = if testnet { "bytick" } else { "bybit" };
        let url = format!("https://{}.{}.com", sub_domain, domain);
        let client = reqwest::Client::new();

        HttpManager {
            api_key,
            api_secret,
            base_url: url,
            recv_window: 5000,
            ignore_codes: vec![],
            max_retries: 10,
            client,
        }
    }
}

impl Manager for HttpManager {
    fn auth(
        &self,
        req_params: &str,
        recv_window: u64,
        timestamp: u64,
    ) -> Result<String, &'static str> {
        if self.api_key.is_empty() || self.api_secret.is_empty() {
            return Err("Authenticated endpoints require keys");
        }

        let param_str = format!(
            "{}{}{}{}",
            timestamp, &self.api_key, recv_window, req_params
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(self.api_secret.as_bytes())
            .expect("HMAC initialization failed");
        mac.update(param_str.as_bytes());
        let signature = mac.finalize().into_bytes();
        Ok(hex::encode(signature))
    }

    fn prepare_payload(&self, method: &str, parameters: HashMap<String, String>) -> String {
        if method == "GET" {
            parameters
                .iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join("&")
        } else {
            json!(parameters).to_string()
        }
    }

    fn generate_timestamp(&self) -> u64 {
        let current_time = SystemTime::now();
        let since_epoch = current_time
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");
        let timestamp = since_epoch.as_secs() as u64 * 1000 + since_epoch.as_millis() as u64;

        timestamp
    }

    async fn submit_request(
        &self,
        method: Method,
        path: &str,
        query: HashMap<String, String>,
        auth: bool,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let mut req_params = String::new();
        let mut recv_window = self.recv_window;

        if auth {
            let timestamp = self.generate_timestamp();
            recv_window = std::cmp::min(recv_window, self.recv_window);
            req_params = self.prepare_payload(method.as_str(), query.clone());
            let signature = self.auth(&req_params, recv_window, timestamp)?;
            println!("{:?}", signature);
            let mut headers = header::HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, "application/json".parse()?);
            headers.insert("X-BAPI-API-KEY", self.api_key.parse()?);
            headers.insert("X-BAPI-SIGN", signature.parse()?);
            headers.insert("X-BAPI-SIGN-TYPE", "2".parse()?);
            headers.insert("X-BAPI-TIMESTAMP", timestamp.to_string().parse()?);
            headers.insert("X-BAPI-RECV-WINDOW", recv_window.to_string().parse()?);

            let request_builder = self
                .client
                .request(method.clone(), format!("{}{}", self.base_url, path))
                .headers(headers);

            let response = if method == Method::GET && !query.is_empty() {
                request_builder.query(&query).send().await?
            } else {
                request_builder.json(&query).send().await?
            };

            let response_text = response.text().await?;
            let s_json: Value = serde_json::from_str(&response_text)?;
            let ret_code = "retCode";
            let ret_msg = "retMsg";

            if let Some(ret_code_val) = s_json.get(ret_code).and_then(Value::as_i64) {
                if ret_code_val != 0 {
                    // Handle error case
                    let ret_msg_val = s_json
                        .get(ret_msg)
                        .and_then(Value::as_str)
                        .unwrap_or("Unknown error");
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!(
                            "InvalidRequestError: {} {} {}. {}",
                            method, path, req_params, ret_msg_val
                        ),
                    )));
                } else {
                    // Handle success case
                    return Ok(s_json);
                }
            }
        }

        Ok(Value::Null)
    }
}