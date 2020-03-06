use std::path::PathBuf;

use crossbeam_channel::Sender;
use serde_json::StreamDeserializer;
use serde_json::Value;

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
pub struct RpcReq {
    #[serde(default, deserialize_with = "deserialize_some")]
    pub id: Option<JsonRpcV2Id>,
    #[serde(default)]
    pub jsonrpc: JsonRpcV2,
    pub method: String,
    pub params: Vec<Value>,
}
impl AsRef<RpcReq> for RpcReq {
    fn as_ref(&self) -> &RpcReq {
        &self
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct RpcRes {
    pub id: JsonRpcV2Id,
    pub jsonrpc: JsonRpcV2,
    #[serde(flatten)]
    pub result: RpcResult,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
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
    pub message: String,
    #[serde(
        default,
        deserialize_with = "deserialize_some",
        skip_serializing_if = "Option::is_none"
    )]
    pub data: Option<Value>,
}

pub fn handle_stdio_rpc(send_side: Sender<PathBuf>) {
    // create serde stream
    let req_stream: StreamDeserializer<_, RpcReq> =
        StreamDeserializer::new(serde_json::de::IoRead::new(std::io::stdin()));
    // for request in stream
    for e_req in req_stream {
        let rpc_result = match e_req {
            Ok(RpcReq {
                id: Some(req_id),
                jsonrpc: _,
                method: method,
                params: params,
            }) => RpcRes {
                id: req_id,
                jsonrpc: Default::default(),
                result: match &*method {
                    "init" => match init(send_side.clone(), params) {
                        Ok(_) => RpcResult::Result(serde_json::json!({})),
                        Err(e) => RpcResult::Error(RpcError {
                            code: todo!(),
                            message: todo!(),
                            data: Some(Value::String(format!("{}", e))),
                        }),
                    },
                    "getmanifest" => {
                        RpcResult::Result(serde_json::json!(include!("manifest.json")))
                    }
                    other => RpcResult::Error(RpcError {
                        code: todo!(),
                        message: todo!(),
                        data: Some(Value::String(format!("{}", other.to_owned()))),
                    }),
                },
            },
            Ok(RpcReq {
                id: None,
                jsonrpc: _,
                method: method,
                params: params,
            }) => {
                continue;
            }
            Err(e) => RpcRes {
                id: JsonRpcV2Id::Null,
                jsonrpc: JsonRpcV2,
                result: RpcResult::Error(RpcError {
                    code: todo!(),
                    message: todo!(),
                    data: Some(Value::String(format!("{}", e))),
                }),
            },
        };
        serde_json::to_writer(std::io::stdout(), &rpc_result);
        print!("\n\n");
    }
}

fn init(send_side: Sender<PathBuf>, mut conf: Vec<Value>) -> Result<(), failure::Error> {
    let arg0 = conf
        .pop()
        .ok_or(failure::format_err!("No arguments supplied"))?;
    let conf: LightningConfig = serde_json::from_value(arg0)?;
    send_side.send(conf.lightning_dir.join(conf.rpc_file));
    Ok(())
}

#[derive(Clone, Debug)]
pub struct LightningConfig {
    lightning_dir: PathBuf,
    rpc_file: String,
    startup: bool,
}

impl<'de> serde::Deserialize<'de> for LightningConfig {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Complete {
            configuration: LightningConfig,
        }
        let complete = Complete::deserialize(d)?;
        Ok(complete.configuration)
    }
}
