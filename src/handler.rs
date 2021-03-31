use std::pin::Pin;
use std::sync::Arc;
use std::{collections::HashMap, future::Future};

use aws_lambda_events::{
    encodings::Body,
    event::apigw::{ApiGatewayProxyRequestContext, ApiGatewayRequestIdentity},
};
use lamedh_http::{request::RequestContext, Handler, Request, RequestExt, Response};
use lamedh_runtime::Context;
use rocket::http::{Header, uri::{Origin, Uri}};
use rocket::local::asynchronous::{Client, LocalRequest, LocalResponse};
use rocket::{Rocket, Route};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::*;
use crate::error::RocketLambError;
use crate::request_ext::RequestExt as _;

/// A Lambda handler for API Gateway events that processes requests using a [Rocket](rocket::Rocket) instance.
pub struct RocketHandler {
    pub(super) lazy_client: Arc<Mutex<LazyClient>>,
    pub(super) config: Arc<Config>,
}

pub(super) enum LazyClient {
    Uninitialized(Option<Rocket>),
    Ready(Arc<Client>),
}

type HandlerError = failure::Error;
type HandlerResponse = Response<Body>;
type HandlerResult = Result<HandlerResponse, HandlerError>;

impl Handler for RocketHandler {
    type Error = HandlerError;
    type Response = HandlerResponse;

    type Fut = Pin<Box<dyn Future<Output = HandlerResult> + 'static>>;

    fn call(&mut self, req: Request, _ctx: Context) -> Self::Fut {
        let config = Arc::clone(&self.config);
        let lazy_client = Arc::clone(&self.lazy_client);
        let fut = async {
            process_request(lazy_client, config, req)
                .await
                .map_err(failure::Error::from)
                .map_err(failure::Error::into)
        };
        Box::pin(fut)
    }
}

fn get_path_and_query(config: &Config, req: &Request) -> String {
    let mut uri = match &config.base_path_behaviour {
        BasePathBehaviour::Include | BasePathBehaviour::RemountAndInclude => dbg!(req.full_path()),
        BasePathBehaviour::Exclude => dbg!(req.api_path().to_owned()),
    };
    let query = req.query_string_parameters();

    let mut separator = '?';
    for (key, _) in query.iter() {
        for value in query.get_all(key).unwrap() {
            uri.push_str(&format!(
                "{}{}={}",
                separator,
                Uri::percent_encode(key),
                Uri::percent_encode(value)
            ));
            separator = '&';
        }
    }
    uri
}

async fn process_request(
    lazy_client: Arc<Mutex<LazyClient>>,
    config: Arc<Config>,
    req: Request,
) -> Result<Response<Body>, RocketLambError> {
    let client = get_client_from_lazy(&lazy_client, &config, &req).await;
    let local_req = create_rocket_request(&client, Arc::clone(&config), req)?;
    let local_res = local_req.dispatch().await;
    create_lambda_response(config, local_res).await
}

async fn get_client_from_lazy(
    lazy_client_lock: &Mutex<LazyClient>,
    config: &Config,
    req: &Request,
) -> Arc<Client> {
    let mut lazy_client = lazy_client_lock.lock().await;
    match &mut *lazy_client {
        LazyClient::Ready(c) => Arc::clone(&c),
        LazyClient::Uninitialized(r) => {
            let r = r
                .take()
                .expect("It should not be possible for this to be None");
            let base_path = req.base_path();
            let client = if config.base_path_behaviour == BasePathBehaviour::RemountAndInclude
                && !base_path.is_empty()
            {
                let origin = Origin::parse(&base_path).expect("Base path needs to be parseable into Origin");
                let routes: Vec<Route> = r.routes().cloned().collect();
                let rocket = r.mount(origin, routes);
                Client::untracked(rocket).await.unwrap()
            } else {
                Client::untracked(r).await.unwrap()
            };
            let client = Arc::new(client);
            let client_clone = Arc::clone(&client);
            *lazy_client = LazyClient::Ready(client);
            client_clone
        }
    }
}

fn create_rocket_request(
    client: &Client,
    config: Arc<Config>,
    req: Request,
) -> Result<LocalRequest, RocketLambError> {
    let method = to_rocket_method(req.method())?;
    let uri = get_path_and_query(&config, &req);
    let mut local_req = client.req(method, uri);
    for (name, value) in req.headers() {
        match value.to_str() {
            Ok(v) => local_req.add_header(Header::new(name.to_string(), v.to_string())),
            Err(_) => return Err(invalid_request!("invalid value for header '{}'", name)),
        }
    }
    for (name, value) in map_request_context_to_headers(req.request_context()) {
        local_req.add_header(Header::new(name, value));
    }
    local_req.set_body(req.into_body());
    Ok(local_req)
}

async fn create_lambda_response(
    config: Arc<Config>,
    local_res: LocalResponse<'_>,
) -> Result<Response<Body>, RocketLambError> {
    let mut builder = Response::builder();
    builder = builder.status(local_res.status().code);
    for h in local_res.headers().iter() {
        builder = builder.header(&h.name.to_string(), &h.value.to_string());
    }

    let response_type = local_res
        .headers()
        .get_one("content-type")
        .unwrap_or_default()
        .split(';')
        .next()
        .and_then(|ct| config.response_types.get(&ct.to_lowercase()))
        .copied()
        .unwrap_or(config.default_response_type);
    let body = match local_res.into_bytes().await {
        Some(b) => match response_type {
            ResponseType::Auto => match String::from_utf8(b) {
                Ok(s) => Body::Text(s),
                Err(e) => Body::Binary(e.into_bytes()),
            },
            ResponseType::Text => Body::Text(
                String::from_utf8(b)
                    .map_err(|_| invalid_response!("failed to read response body as UTF-8"))?,
            ),
            ResponseType::Binary => Body::Binary(b),
        },
        None => Body::Empty,
    };

    builder.body(body).map_err(|e| invalid_response!("{}", e))
}

fn to_rocket_method(method: &http::Method) -> Result<rocket::http::Method, RocketLambError> {
    use http::Method as H;
    use rocket::http::Method::*;
    Ok(match *method {
        H::GET => Get,
        H::PUT => Put,
        H::POST => Post,
        H::DELETE => Delete,
        H::OPTIONS => Options,
        H::HEAD => Head,
        H::TRACE => Trace,
        H::CONNECT => Connect,
        H::PATCH => Patch,
        _ => return Err(invalid_request!("unknown method '{}'", method)),
    })
}

fn map_request_context_to_headers(req_ctx: RequestContext) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    match req_ctx {
        RequestContext::ApiGatewayV1(ctx) => {
            add_api_gw_v1_headers(ctx, &mut headers);
        }
        RequestContext::Alb(ctx) => {
            if ctx.elb.target_group_arn.is_some() {
                headers.insert(
                    "x-amz-req-ctx-target-group-arn".to_string(),
                    ctx.elb.target_group_arn.unwrap(),
                );
            }
        }
        RequestContext::ApiGatewayV2(_) => (), //I don't think V2 can be a proxy?
    };
    headers
}

fn add_api_gw_v1_headers(
    ctx: ApiGatewayProxyRequestContext<Value>,
    headers: &mut HashMap<String, String>,
) {
    if ctx.account_id.is_some() {
        headers
            .insert("x-amz-req-ctx-account-id".to_string(), ctx.account_id.unwrap());
    }
    if ctx.resource_id.is_some() {
        headers.insert(
            "x-amz-req-ctx-resource-id".to_string(),
            ctx.resource_id.unwrap(),
        );
    }
    if ctx.stage.is_some() {
        headers.insert("x-amz-req-ctx-stage".to_string(), ctx.stage.unwrap());
    }
    if ctx.domain_name.is_some() {
        headers.insert(
            "x-amz-req-ctx-domain-name".to_string(),
            ctx.domain_name.unwrap(),
        );
    }
    if ctx.domain_prefix.is_some() {
        headers.insert(
            "x-amz-req-ctx-domain-prefix".to_string(),
            ctx.domain_prefix.unwrap(),
        );
    }
    if ctx.protocol.is_some() {
        headers.insert("x-amz-req-ctx-protocol".to_string(), ctx.protocol.unwrap());
    }
    add_identity_ctx_headers(headers, ctx.identity);
    if ctx.resource_path.is_some() {
        headers.insert(
            "x-amz-req-ctx-resource-path".to_string(),
            ctx.resource_path.unwrap(),
        );
    }
    ctx.authorizer.iter().for_each(|(k, v)| {
        headers.insert(format!("x-amz-req-ctx-auth-{}", k), v.to_string());
    });
    if ctx.request_time.is_some() {
        headers.insert(
            "x-amz-req-ctx-request-time".to_string(),
            ctx.request_time.unwrap(),
        );
    }
    if ctx.apiid.is_some() {
        headers.insert("x-amz-req-ctx-apiid".to_string(), ctx.apiid.unwrap());
    }
}

fn add_identity_ctx_headers(
    headers: &mut HashMap<String, String>,
    identity: ApiGatewayRequestIdentity,
) {
    if identity.cognito_identity_pool_id.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-cognito-identity-pool-id".to_string(),
            identity.cognito_identity_pool_id.unwrap(),
        );
    }
    if identity.account_id.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-account-id".to_string(),
            identity.account_id.unwrap(),
        );
    }
    if identity.cognito_identity_id.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-cognito-identity-id".to_string(),
            identity.cognito_identity_id.unwrap(),
        );
    }
    if identity.caller.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-caller".to_string(),
            identity.caller.unwrap(),
        );
    }
    if identity.api_key.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-api-key".to_string(),
            identity.api_key.unwrap(),
        );
    }
    if identity.access_key.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-access-key".to_string(),
            identity.access_key.unwrap(),
        );
    }
    if identity.source_ip.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-source-ip".to_string(),
            identity.source_ip.unwrap(),
        );
    }
    if identity.cognito_authentication_type.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-cognito-authentication-type".to_string(),
            identity.cognito_authentication_type.unwrap(),
        );
    }
    if identity.cognito_authentication_provider.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-cognito-authentication-provider".to_string(),
            identity.cognito_authentication_provider.unwrap(),
        );
    }
    if identity.user_arn.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-user-arn".to_string(),
            identity.user_arn.unwrap(),
        );
    }
    if identity.user_agent.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-user-agent".to_string(),
            identity.user_agent.unwrap(),
        );
    }
    if identity.user.is_some() {
        headers.insert("x-amz-req-ctx-id-user".to_string(), identity.user.unwrap());
    }
}
