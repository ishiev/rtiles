use std::convert::Infallible;

use rocket::{
    request::{FromRequest, Outcome},
    Request,
};

/// Model identity
#[derive(Default, Debug, Hash, PartialEq, Eq, Clone)]
pub struct Model {
    pub object: Option<String>, // None means all objects and all models
    pub name: Option<String>,   // None means all models of a given object
}

impl Model {
    pub fn new(object: Option<&str>, name: Option<&str>) -> Self {
        Model {
            object: object.map(str::to_owned),
            name: name.map(str::to_owned),
        }
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for Model {
    type Error = Infallible;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let model = Model {
            object: req.param(1).map(|x| x.unwrap_or_default()),
            name: req.param(2).map(|x| x.unwrap_or_default()),
        };

        Outcome::Success(model)
    }
}
