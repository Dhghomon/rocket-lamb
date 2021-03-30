use crate::config::*;
use crate::error::RocketLambError;
use crate::request_ext::RequestExt as _;
use lambda_http::{
    request::{Identity, RequestContext},
    Body, Handler, Request, RequestExt, Response,
};
use lambda_runtime::{error::HandlerError, Context};
use rocket::http::{uri::Uri, Header};
use rocket::local::{Client, LocalRequest, LocalResponse};
use rocket::{Rocket, Route};
use std::{collections::HashMap, mem};

/// A Lambda handler for API Gateway events that processes requests using a [Rocket](rocket::Rocket) instance.
pub struct RocketHandler {
    pub(super) client: LazyClient,
    pub(super) config: Config,
}

pub(super) enum LazyClient {
    Placeholder,
    Uninitialized(Rocket),
    Ready(Client),
}

impl Handler<Response<Body>> for RocketHandler {
    fn run(&mut self, req: Request, _ctx: Context) -> Result<Response<Body>, HandlerError> {
        self.ensure_client_ready(&req);
        self.process_request(req)
            .map_err(failure::Error::from)
            .map_err(failure::Error::into)
    }
}

impl RocketHandler {
    fn ensure_client_ready(&mut self, req: &Request) {
        match self.client {
            ref mut lazy_client @ LazyClient::Uninitialized(_) => {
                let uninitialized_client = mem::replace(lazy_client, LazyClient::Placeholder);
                let mut rocket = match uninitialized_client {
                    LazyClient::Uninitialized(rocket) => rocket,
                    _ => unreachable!("LazyClient must be uninitialized at this point."),
                };
                if self.config.base_path_behaviour == BasePathBehaviour::RemountAndInclude {
                    let base_path = req.base_path();
                    if !base_path.is_empty() {
                        let routes: Vec<Route> = rocket.routes().cloned().collect();
                        rocket = rocket.mount(&base_path, routes);
                    }
                }
                let client = Client::untracked(rocket).unwrap();
                self.client = LazyClient::Ready(client);
            }
            LazyClient::Ready(_) => {}
            LazyClient::Placeholder => panic!("LazyClient has previously begun initialiation."),
        }
    }

    fn client(&self) -> &Client {
        match &self.client {
            LazyClient::Ready(client) => client,
            _ => panic!("Rocket client wasn't ready. ensure_client_ready should have been called!"),
        }
    }

    fn process_request(&self, req: Request) -> Result<Response<Body>, RocketLambError> {
        let local_req = self.create_rocket_request(req)?;
        let local_res = local_req.dispatch();
        self.create_lambda_response(local_res)
    }

    fn create_rocket_request(&self, req: Request) -> Result<LocalRequest, RocketLambError> {
        let method = to_rocket_method(req.method())?;
        let uri = self.get_path_and_query(&req);
        let mut local_req = self.client().req(method, uri);
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

    fn create_lambda_response(
        &self,
        mut local_res: LocalResponse,
    ) -> Result<Response<Body>, RocketLambError> {
        let mut builder = Response::builder();
        builder.status(local_res.status().code);
        for h in local_res.headers().iter() {
            builder.header(&h.name.to_string(), &h.value.to_string());
        }

        let response_type = local_res
            .headers()
            .get_one("content-type")
            .unwrap_or_default()
            .split(';')
            .next()
            .and_then(|ct| self.config.response_types.get(&ct.to_lowercase()))
            .copied()
            .unwrap_or(self.config.default_response_type);
        let body = match (local_res.body(), response_type) {
            (Some(b), ResponseType::Auto) => {
                let bytes = b
                    .into_bytes()
                    .ok_or_else(|| invalid_response!("failed to read response body"))?;
                match String::from_utf8(bytes) {
                    Ok(s) => Body::Text(s),
                    Err(e) => Body::Binary(e.into_bytes()),
                }
            }
            (Some(b), ResponseType::Text) => Body::Text(
                b.into_string()
                    .ok_or_else(|| invalid_response!("failed to read response body as UTF-8"))?,
            ),
            (Some(b), ResponseType::Binary) => Body::Binary(
                b.into_bytes()
                    .ok_or_else(|| invalid_response!("failed to read response body"))?,
            ),
            (None, _) => Body::Empty,
        };

        builder.body(body).map_err(|e| invalid_response!("{}", e))
    }

    fn get_path_and_query(&self, req: &Request) -> String {
        let mut uri = match self.config.base_path_behaviour {
            BasePathBehaviour::Include | BasePathBehaviour::RemountAndInclude => req.full_path(),
            BasePathBehaviour::Exclude => req.api_path().to_owned(),
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
        RequestContext::ApiGateway {
            account_id,
            resource_id,
            stage,
            request_id,
            resource_path: _,
            http_method: _,
            authorizer,
            api_id,
            identity,
        } => {
            headers.insert("x-amz-req-ctx-account-id".to_string(), account_id);
            headers.insert("x-amz-req-ctx-resource-id".to_string(), resource_id);
            headers.insert("x-amz-req-ctx-stage".to_string(), stage);
            headers.insert("x-amz-req-ctx-request-id".to_string(), request_id);
            headers.insert("x-amz-req-ctx-api-id".to_string(), api_id);
            authorizer.iter().for_each(|(k, v)| {
                headers.insert(format!("x-amz-req-ctx-auth-{}", k), v.to_string());
            });
            add_identity_ctx_headers(&mut headers, identity);
        }
        RequestContext::Alb { elb } => {
            headers.insert(
                "x-amz-req-ctx-target-group-arn".to_string(),
                elb.target_group_arn,
            );
        }
    }
    headers
}

fn add_identity_ctx_headers(headers: &mut HashMap<String, String>, identity: Identity) {
    headers.insert("x-amz-req-ctx-id-source-ip".to_string(), identity.source_ip);
    if identity.cognito_identity_id.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-cognito-identity-id".to_string(),
            identity.cognito_identity_id.unwrap(),
        );
    }
    if identity.cognito_identity_pool_id.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-cognito-identity-pool-id".to_string(),
            identity.cognito_identity_pool_id.unwrap(),
        );
    }
    if identity.cognito_authentication_provider.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-cognito-authentication-provider".to_string(),
            identity.cognito_authentication_provider.unwrap(),
        );
    }
    if identity.cognito_authentication_type.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-cognito-authentication-type".to_string(),
            identity.cognito_authentication_type.unwrap(),
        );
    }
    if identity.account_id.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-account-id".to_string(),
            identity.account_id.unwrap(),
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
    if identity.user.is_some() {
        headers.insert("x-amz-req-ctx-id-user".to_string(), identity.user.unwrap());
    }
    if identity.user_agent.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-user-agent".to_string(),
            identity.user_agent.unwrap(),
        );
    }
    if identity.user_arn.is_some() {
        headers.insert(
            "x-amz-req-ctx-id-user-arn".to_string(),
            identity.user_arn.unwrap(),
        );
    }
}
