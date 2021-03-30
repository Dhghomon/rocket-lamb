#[macro_use]
extern crate rocket;

use aws_lambda_events::encodings::Body;
use lamedh_http::{Handler, Request};
use lamedh_runtime::Context;
use rocket::http::uri::Origin;
use rocket_lamb::{BasePathBehaviour, RocketExt, RocketHandler};
use std::error::Error;
use std::fs::File;

#[catch(404)]
fn not_found(req: &rocket::Request) -> String {
    req.uri().to_string()
}

#[get("/path")]
fn get_path<'r>(origin: &'r Origin<'r>) -> &'r str {
    origin.path()
}

async fn make_rocket(base_path_behaviour: BasePathBehaviour) -> RocketHandler {
    rocket::ignite()
        .mount("/", routes![get_path])
        .register(catchers![not_found])
        .lambda()
        .base_path_behaviour(base_path_behaviour)
        .into_handler()
        .await
}

fn get_request(json_file: &str) -> Result<Request, Box<dyn Error>> {
    let file = File::open(format!("tests/requests/{}.json", json_file))?;
    Ok(lamedh_http::request::from_reader(file)?)
}

#[tokio::test]
async fn api_gateway() {
    let mut handler = make_rocket(BasePathBehaviour::RemountAndInclude).await;

    let req = get_request("path_api_gateway").unwrap();
    let res = handler.call(req, Context::default()).await.unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(*res.body(), Body::Text("/Prod/path/".to_string()));
}

#[tokio::test]
async fn api_gateway_include_base() {
    let mut handler = make_rocket(BasePathBehaviour::Include).await;

    let req = get_request("path_api_gateway").unwrap();
    let res = handler.call(req, Context::default()).await.unwrap();

    assert_eq!(res.status(), 404);
    assert_eq!(*res.body(), Body::Text("/Prod/path/".to_string()));
}

#[tokio::test]
async fn api_gateway_exclude_base() {
    let mut handler = make_rocket(BasePathBehaviour::Exclude).await;

    let req = get_request("path_api_gateway").unwrap();
    let res = handler.call(req, Context::default()).await.unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(*res.body(), Body::Text("/path/".to_string()));
}

#[tokio::test]
async fn custom_domain() {
    let mut handler = make_rocket(BasePathBehaviour::RemountAndInclude).await;

    let req = get_request("path_custom_domain").unwrap();
    let res = handler.call(req, Context::default()).await.unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(*res.body(), Body::Text("/path/".to_string()));
}

#[tokio::test]
async fn custom_domain_include_empty_base() {
    let mut handler = make_rocket(BasePathBehaviour::Include).await;

    let req = get_request("path_custom_domain").unwrap();
    let res = handler.call(req, Context::default()).await.unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(*res.body(), Body::Text("/path/".to_string()));
}

#[tokio::test]
async fn custom_domain_exclude_empty_base() {
    let mut handler = make_rocket(BasePathBehaviour::Exclude).await;

    let req = get_request("path_custom_domain").unwrap();
    let res = handler.call(req, Context::default()).await.unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(*res.body(), Body::Text("/path/".to_string()));
}

#[tokio::test]
async fn custom_domain_with_base_path() {
    let mut handler = make_rocket(BasePathBehaviour::RemountAndInclude).await;

    let req = get_request("path_custom_domain_with_base").unwrap();
    let res = handler.call(req, Context::default()).await.unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(*res.body(), Body::Text("/base-path/path/".to_string()));
}

#[tokio::test]
async fn custom_domain_with_base_path_include() {
    let mut handler = make_rocket(BasePathBehaviour::Include).await;

    let req = get_request("path_custom_domain_with_base").unwrap();
    let res = handler.call(req, Context::default()).await.unwrap();

    assert_eq!(res.status(), 404);
    assert_eq!(*res.body(), Body::Text("/base-path/path/".to_string()));
}

#[tokio::test]
async fn custom_domain_with_base_path_exclude() {
    let mut handler = make_rocket(BasePathBehaviour::Exclude).await;

    let req = get_request("path_custom_domain_with_base").unwrap();
    let res = handler.call(req, Context::default()).await.unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(*res.body(), Body::Text("/path/".to_string()));
}

#[tokio::test]
async fn application_load_balancer() {
    let mut handler = make_rocket(BasePathBehaviour::RemountAndInclude).await;

    let req = get_request("path_alb").unwrap();
    let res = handler.call(req, Context::default()).await.unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(*res.body(), Body::Text("/path/".to_string()));
}

#[tokio::test]
async fn application_load_balancer_include_empty_base() {
    let mut handler = make_rocket(BasePathBehaviour::Include).await;

    let req = get_request("path_alb").unwrap();
    let res = handler.call(req, Context::default()).await.unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(*res.body(), Body::Text("/path/".to_string()));
}

#[tokio::test]
async fn application_load_balancer_exclude_empty_base() {
    let mut handler = make_rocket(BasePathBehaviour::Exclude).await;

    let req = get_request("path_alb").unwrap();
    let res = handler.call(req, Context::default()).await.unwrap();

    assert_eq!(res.status(), 200);
    assert_eq!(*res.body(), Body::Text("/path/".to_string()));
}
