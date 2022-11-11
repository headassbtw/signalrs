use crate::{
    client::{stream_ext::SignalRStreamExt, InvocationStream},
    protocol::{Invocation, StreamInvocation, StreamItem},
};

use super::{
    client::{ResponseStream, SignalRClient},
    messages::{ClientMessage, MessageEncoding},
    IntoInvocationPart, InvocationPart, SignalRClientError,
};
use futures::{Stream, StreamExt};
use serde::{de::DeserializeOwned, Serialize};
use uuid::Uuid;

pub struct SendBuilder<'a> {
    client: &'a SignalRClient,
    method: String,
    encoding: MessageEncoding,
    arguments: Vec<serde_json::Value>,
    streams: Vec<ClientStream>,
}

struct ClientStream {
    stream_id: String,
    items: Box<dyn Stream<Item = ClientMessage> + Unpin + Send>,
}

impl<'a> SendBuilder<'a> {
    pub fn new(client: &'a SignalRClient, method: impl ToString) -> Self {
        SendBuilder {
            client,
            method: method.to_string(),
            encoding: MessageEncoding::Json,
            arguments: Default::default(),
            streams: Default::default(),
        }
    }

    /// Adds ordered argument to invocation
    pub fn arg<A, B>(mut self, arg: A) -> Result<Self, SignalRClientError>
    where
        A: IntoInvocationPart<B> + Send + 'static,
        B: Serialize + Send + 'static,
    {
        match arg.into() {
            InvocationPart::Argument(arg) => self.arguments.push(serde_json::to_value(arg)?),
            InvocationPart::Stream(stream) => {
                let stream_id = Uuid::new_v4().to_string();
                let client_stream = into_client_stream(stream_id, stream, self.encoding);
                self.streams.push(client_stream);
            }
        };

        return Ok(self);

        fn into_client_stream<A: Serialize + Send + 'static>(
            stream_id: String,
            input: InvocationStream<A>,
            encoding: MessageEncoding,
        ) -> ClientStream {
            let items = input
                .zip(futures::stream::repeat(stream_id.clone()))
                .map(|(i, id)| StreamItem::new(id, i))
                .map(move |i| encoding.serialize(i))
                .append_completion(stream_id.clone(), encoding);

            ClientStream {
                stream_id,
                items: Box::new(items),
            }
        }
    }

    pub async fn send(self) -> Result<(), SignalRClientError> {
        let arguments = args_as_option(self.arguments);

        let mut invocation = Invocation::non_blocking(self.method, arguments);
        invocation.with_streams(get_stream_ids(&self.streams));

        let serialized = self.encoding.serialize(&invocation)?;

        self.client.send_message(serialized).await?;
        SignalRClient::send_streams(
            self.client.get_transport_handle(),
            into_actual_streams(self.streams),
        )
        .await
    }

    pub async fn invoke<T: DeserializeOwned>(self) -> Result<T, SignalRClientError> {
        let invocation_id = Uuid::new_v4().to_string();
        let arguments = args_as_option(self.arguments);

        let mut invocation = Invocation::non_blocking(self.method, arguments);
        invocation.with_invocation_id(invocation_id.clone());
        invocation.with_streams(get_stream_ids(&self.streams));

        let serialized = self.encoding.serialize(&invocation)?;

        self.client
            .invoke::<T>(invocation_id, serialized, into_actual_streams(self.streams))
            .await
    }

    pub async fn invoke_stream<T: DeserializeOwned>(
        self,
    ) -> Result<ResponseStream<'a, T>, SignalRClientError> {
        let invocation_id = Uuid::new_v4().to_string();

        let mut invocation =
            StreamInvocation::new(invocation_id.clone(), self.method, Some(self.arguments));
        invocation.with_streams(get_stream_ids(&self.streams));

        let serialized = self.encoding.serialize(&invocation)?;

        let response_stream = self
            .client
            .invoke_stream::<T>(invocation_id, serialized, into_actual_streams(self.streams))
            .await?;

        Ok(response_stream)
    }
}

fn args_as_option(arguments: Vec<serde_json::Value>) -> Option<Vec<serde_json::Value>> {
    if arguments.is_empty() {
        None
    } else {
        Some(arguments)
    }
}

fn get_stream_ids(streams: &[ClientStream]) -> Vec<String> {
    streams.iter().map(|s| s.get_stream_id()).collect()
}

fn into_actual_streams(
    streams: Vec<ClientStream>,
) -> Vec<Box<dyn Stream<Item = ClientMessage> + Unpin + Send>> {
    streams.into_iter().map(|s| s.items).collect()
}

impl ClientStream {
    pub fn get_stream_id(&self) -> String {
        self.stream_id.clone()
    }
}
