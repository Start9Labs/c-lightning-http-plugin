use std::path::PathBuf;

use crossbeam_channel::Sender;
use serde_json::StreamDeserializer;
use serde_json::Value;

use crate::init_info::InitInfo;

fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
pub enum JsonRpcV2Id {
    Num(serde_json::Number),
    Str(String),
    Null,
}

#[derive(Clone, Debug)]
pub struct JsonRpcV2;
impl Default for JsonRpcV2 {
    fn default() -> Self {
        JsonRpcV2
    }
}
impl serde::Serialize for JsonRpcV2 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("2.0")
    }
}
impl<'de> serde::Deserialize<'de> for JsonRpcV2 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let version: String = serde::Deserialize::deserialize(deserializer)?;
        match version.as_str() {
            "2.0" => (),
            a => {
                return Err(serde::de::Error::custom(format!(
                    "invalid RPC version: {}",
                    a
                )))
            }
        }
        Ok(JsonRpcV2)
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
pub enum RpcParams {
    ByPosition(Vec<Value>),
    ByName(serde_json::Map<String, Value>),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct RpcReq {
    #[serde(default, deserialize_with = "deserialize_some")]
    pub id: Option<JsonRpcV2Id>,
    #[serde(default)]
    pub jsonrpc: JsonRpcV2,
    pub method: String,
    pub params: RpcParams,
}
impl AsRef<RpcReq> for RpcReq {
    fn as_ref(&self) -> &RpcReq {
        &self
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct RpcRes {
    pub id: JsonRpcV2Id,
    pub jsonrpc: JsonRpcV2,
    #[serde(flatten)]
    pub result: RpcResult,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RpcResult {
    Result(Value),
    Error(RpcError),
}
impl RpcResult {
    pub fn res(self) -> Result<Value, RpcError> {
        self.into()
    }
}
impl From<RpcResult> for Result<Value, RpcError> {
    fn from(r: RpcResult) -> Self {
        match r {
            RpcResult::Result(a) => Ok(a),
            RpcResult::Error(e) => Err(e),
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct RpcError {
    pub code: serde_json::Number,
    pub message: &'static str,
    #[serde(
        default,
        deserialize_with = "deserialize_some",
        skip_serializing_if = "Option::is_none"
    )]
    pub data: Option<Value>,
}

pub fn handle_stdio_rpc(send_side: Sender<InitInfo>) {
    // create serde stream
    let req_stream: StreamDeserializer<_, RpcReq> =
        StreamDeserializer::new(serde_json::de::IoRead::new(std::io::stdin()));
    // for request in stream
    for e_req in req_stream {
        let rpc_result = match e_req {
            Ok(RpcReq {
                id: Some(req_id),
                method,
                params,
                ..
            }) => RpcRes {
                id: req_id,
                jsonrpc: Default::default(),
                result: match &*method {
                    "init" => match init(send_side.clone(), params) {
                        Ok(_) => RpcResult::Result(serde_json::json!({})),
                        Err(e) => RpcResult::Error(RpcError {
                            code: serde_json::Number::from(1),
                            message: "error processing init",
                            data: Some(Value::String(format!("{}", e))),
                        }),
                    },
                    "getmanifest" => RpcResult::Result(serde_json::json!({
                        "options": [
                            {
                                "name": "http-user",
                                "type": "string",
                                "default": "lightning",
                                "description": "Basic-Auth user header for http authentication"
                            },
                            {
                                "name": "http-pass",
                                "type": "string",
                                "default": "",
                                "description": "Basic-Auth password header for http authentication, not setting this will result in requests being rejected"
                            },
                            {
                                "name": "http-port",
                                "type": "int",
                                "default": 8080,
                                "description": "Http port for web server listening"
                            }
                        ],
                        "rpcmethods": [],
                        "subscriptions": [],
                        "hooks": [],
                        "features": {
                            "node": "00000000",
                            "init": "00000000",
                            "invoice": "00000000"
                        },
                        "dynamic": true
                    })),
                    other => RpcResult::Error(RpcError {
                        code: serde_json::Number::from(2),
                        message: "unknown method",
                        data: Some(Value::String(format!("{}", other.to_owned()))),
                    }),
                },
            },
            Ok(RpcReq { id: None, .. }) => {
                continue;
            }
            Err(e) => RpcRes {
                id: JsonRpcV2Id::Null,
                jsonrpc: JsonRpcV2,
                result: RpcResult::Error(RpcError {
                    code: serde_json::Number::from(3),
                    message: "parse error",
                    data: Some(Value::String(format!("{}", e))),
                }),
            },
        };
        serde_json::to_writer(std::io::stdout(), &rpc_result)
            .unwrap_or_else(|e| eprintln!("error writing rpc response: {}", e));
        match rpc_result.result {
            RpcResult::Error(e) => eprintln!("{:?}", e),
            _ => (),
        };
        print!("\n\n");
    }
}

fn init(send_side: Sender<InitInfo>, conf: RpcParams) -> Result<(), failure::Error> {
    let arg0 = match conf {
        RpcParams::ByPosition(mut a) => a
            .pop()
            .ok_or(failure::format_err!("no arguments supplied"))?,
        RpcParams::ByName(a) => serde_json::Value::Object(a),
    };
    let conf: LightningInit = serde_json::from_value(arg0)?;
    send_side
        .send(conf.into())
        .unwrap_or_else(|e| eprintln!("SEND ERROR: {}", e)); // ignore send error: means the reciever has already received and been dropped
    Ok(())
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LightningInit {
    options: LightningOptions,
    configuration: LightningConfig,
}

impl From<LightningInit> for InitInfo {
    fn from(li: LightningInit) -> Self {
        InitInfo {
            socket_path: li
                .configuration
                .lightning_dir
                .join(li.configuration.rpc_file),
            auth_header: if li.options.http_pass.is_empty() {
                None
            } else {
                Some(format!(
                    "Basic {}",
                    base64::encode(&format!(
                        "{}:{}",
                        li.options.http_user, li.options.http_pass
                    ))
                ))
            },
            http_port: li.options.http_port,
        }
    }
}

fn default_user() -> String {
    "lightning".to_owned()
}

fn default_pass() -> String {
    "".to_owned()
}

fn default_port() -> u16 {
    8080
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LightningOptions {
    #[serde(default = "default_user")]
    http_user: String,
    #[serde(default = "default_pass")]
    http_pass: String,
    #[serde(default = "default_port")]
    #[serde(deserialize_with = "deser_str_num")]
    http_port: u16,
}

fn deser_str_num<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u16, D::Error> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum StrNum {
        Str(String),
        Num(u16),
    }
    let sn: StrNum = serde::Deserialize::deserialize(deserializer)?;
    Ok(match sn {
        StrNum::Str(s) => s.parse().map_err(|e| serde::de::Error::custom(e))?,
        StrNum::Num(n) => n,
    })
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LightningConfig {
    lightning_dir: PathBuf,
    rpc_file: String,
    startup: bool,
}
