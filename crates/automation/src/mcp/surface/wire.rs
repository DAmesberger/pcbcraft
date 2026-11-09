//! What sits between the line transport and rmcp: the `2026-07-28`-only
//! rules rmcp's lifecycle leaves to the server.
//!
//! - **`initialize`** is answered with `-32022` listing `2026-07-28`, at any
//!   point, and the connection stays open. rmcp would answer it too, but as
//!   the first message it then ends the connection, so a client that falls
//!   back to `server/discover` after the error could not.
//! - **`ping`** is not in `2026-07-28` and is answered `-32601` at any point;
//!   rmcp answers it before the first request.
//! - **Before the first request** other than `server/discover`, a
//!   notification or a response refers to nothing and is dropped; rmcp would
//!   end the connection.
//! - **Every result** carries `io.modelcontextprotocol/serverInfo` in its
//!   `_meta`, as the spec asks; rmcp adds it to discovery only.
use super::{Server, VERSIONS};
use rmcp::RoleServer;
use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, ErrorData, JsonRpcMessage, MetaObject, PingRequestMethod,
    ServerJsonRpcMessage, ServerResult,
};
use rmcp::transport::Transport;
use serde_json::Value;
use std::future::Future;

/// The `_meta` key results name their server under.
pub const SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// A transport that keeps rmcp to `2026-07-28`.
pub struct ModernOnly<T> {
    inner: T,
    /// Whether a request other than discovery has been passed on, which
    /// starts rmcp's request loop.
    started: bool,
    server: Value,
}

impl<T> ModernOnly<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            started: false,
            server: serde_json::to_value(Server::implementation()).unwrap_or(Value::Null),
        }
    }
}

/// What to do with one incoming message.
enum Verdict {
    Pass,
    Drop,
    Answer(Box<ServerJsonRpcMessage>),
}

fn verdict(message: &ClientJsonRpcMessage, started: bool) -> Verdict {
    match message {
        JsonRpcMessage::Request(request) => match &request.request {
            ClientRequest::InitializeRequest(initialize) => {
                Verdict::Answer(Box::new(ServerJsonRpcMessage::error(
                    ErrorData::unsupported_protocol_version(
                        initialize.params.protocol_version.clone(),
                        VERSIONS,
                    ),
                    Some(request.id.clone()),
                )))
            }
            ClientRequest::PingRequest(_) => {
                Verdict::Answer(Box::new(ServerJsonRpcMessage::error(
                    ErrorData::method_not_found::<PingRequestMethod>(),
                    Some(request.id.clone()),
                )))
            }
            _ => Verdict::Pass,
        },
        _ if started => Verdict::Pass,
        _ => Verdict::Drop,
    }
}

impl<T> Transport<RoleServer> for ModernOnly<T>
where
    T: Transport<RoleServer>,
{
    type Error = T::Error;

    fn send(
        &mut self,
        mut item: ServerJsonRpcMessage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        if let JsonRpcMessage::Response(response) = &mut item {
            stamp(&mut response.result, &self.server);
        }
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
        loop {
            let message = self.inner.receive().await?;
            match verdict(&message, self.started) {
                Verdict::Pass => {
                    if let JsonRpcMessage::Request(request) = &message
                        && !matches!(request.request, ClientRequest::DiscoverRequest(_))
                    {
                        self.started = true;
                    }
                    return Some(message);
                }
                Verdict::Drop => {}
                Verdict::Answer(reply) => {
                    if self.inner.send(*reply).await.is_err() {
                        return None;
                    }
                }
            }
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

/// Puts `server` into the result's `_meta`, where the result has one.
fn stamp(result: &mut ServerResult, server: &Value) {
    let meta = match result {
        ServerResult::DiscoverResult(r) => &mut r.meta,
        ServerResult::InitializeResult(r) => &mut r.meta,
        ServerResult::CompleteResult(r) => &mut r.meta,
        ServerResult::GetPromptResult(r) => &mut r.meta,
        ServerResult::ListPromptsResult(r) => &mut r.meta,
        ServerResult::ListResourcesResult(r) => &mut r.meta,
        ServerResult::ListResourceTemplatesResult(r) => &mut r.meta,
        ServerResult::ReadResourceResult(r) => &mut r.meta,
        ServerResult::ListToolsResult(r) => &mut r.meta,
        ServerResult::ListSkillsResult(r) => &mut r.meta,
        ServerResult::GetSkillResult(r) => &mut r.meta,
        ServerResult::CreateTaskResult(r) => &mut r.meta,
        ServerResult::GetTaskResult(r) => &mut r.meta,
        ServerResult::CallToolResult(r) => &mut r.meta,
        ServerResult::InputRequiredResult(r) => &mut r.meta,
        ServerResult::TaskAckResult(r) => &mut r.meta,
        // The listen result has its own metadata, which rmcp stamps; the
        // rest carry none.
        _ => return,
    };
    meta.get_or_insert_with(MetaObject::default)
        .0
        .insert(SERVER_INFO.to_owned(), server.clone());
}
