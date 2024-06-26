use crate::{
    config,
    db::{Connection, OptTime},
    utils::ping,
};
use eyre::Result;
use rocket::{
    get, post, routes,
    serde::{json::Json, Deserialize, Serialize},
};
use std::{collections::HashMap, env, str::FromStr, time::SystemTime};
use url::Url;

use super::{metrics::MountMetrics, DB, METRICS, TX};

#[derive(Serialize)]
struct Response {
    mollysocket: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ConnectionData {
    pub uuid: String,
    pub device_id: u32,
    pub password: String,
    pub endpoint: String,
}

#[derive(Debug)]
enum RegistrationStatus {
    New,
    Updated,
    Running,
    Forbidden,
    InvalidUuid,
    InvalidEndpoint,
    InternalError,
}

impl From<RegistrationStatus> for String {
    fn from(r: RegistrationStatus) -> Self {
        match r {
            RegistrationStatus::New | RegistrationStatus::Updated | RegistrationStatus::Running => {
                "ok"
            }
            RegistrationStatus::Forbidden => "forbidden",
            RegistrationStatus::InvalidUuid => "invalid_uuid",
            RegistrationStatus::InvalidEndpoint => "invalid_endpoint",
            RegistrationStatus::InternalError => "internal_error",
        }
        .into()
    }
}

#[get("/")]
fn discover() -> Json<Response> {
    gen_rep(HashMap::new())
}

#[post("/", format = "application/json", data = "<co_data>")]
async fn register(co_data: Json<ConnectionData>) -> Json<Response> {
    let mut status = registration_status(&co_data).await;
    match status {
        RegistrationStatus::New => {
            if new_connection(&co_data).is_ok() {
                log::debug!("Connection succeeded");
                if let Err(e) = ping(Url::from_str(&co_data.endpoint).unwrap()).await {
                    log::warn!(
                        "Cound not ping the new connection (uuid={}): {e:?}",
                        &co_data.uuid
                    );
                }
            } else {
                log::debug!("Could not start new connection");
                status = RegistrationStatus::InternalError;
            }
        }
        RegistrationStatus::Updated => {
            if new_connection(&co_data).is_ok() {
                log::debug!("Connection succeeded");
            } else {
                log::debug!("Could not start new connection");
                status = RegistrationStatus::InternalError;
            }
        }
        RegistrationStatus::Forbidden => {
            log::debug!("Connection is currently forbidden");
            if let Ok(co) = DB.get(&co_data.uuid) {
                if co.device_id != co_data.device_id || co.password != co_data.password {
                    if new_connection(&co_data).is_ok() {
                        log::debug!("Connection succeeded");
                        status = RegistrationStatus::Updated;
                        METRICS.forbiddens.dec();
                    } else {
                        log::debug!("Could not start new connection");
                        status = RegistrationStatus::InternalError;
                    }
                }
            } else {
                log::debug!("Could not get info in DB about the connection");
                status = RegistrationStatus::InternalError;
            }
        }
        RegistrationStatus::Running => {
            //TODO: Update last registration for ::Running

            // If the connection is "Running" then the device creds still exists,
            // if the user register on another server or delete the linked device,
            // then the connection ends with a 403 Forbidden
            // If the connection is for an invalid uuid or an error occured : we ignore it
        }
        RegistrationStatus::InvalidEndpoint | RegistrationStatus::InvalidUuid => (),
        _ => {
            log::debug!("Status unknown: {status:?}");
            status = RegistrationStatus::InternalError;
        }
    }
    log::debug!("Status: {status:?}");
    gen_rep(HashMap::from([(
        String::from("status"),
        String::from(status),
    )]))
}

fn new_connection(co_data: &Json<ConnectionData>) -> Result<()> {
    let co = Connection {
        uuid: co_data.uuid.clone(),
        device_id: co_data.device_id,
        password: co_data.password.clone(),
        endpoint: co_data.endpoint.clone(),
        forbidden: false,
        last_registration: OptTime::from(SystemTime::now()),
    };
    DB.add(&co).unwrap();
    if let Some(tx) = &*TX.lock().unwrap() {
        let _ = tx.unbounded_send(co);
    }
    Ok(())
}

async fn registration_status(co_data: &ConnectionData) -> RegistrationStatus {
    let endpoint_valid = config::is_endpoint_valid(&co_data.endpoint).await;
    let uuid_valid = config::is_uuid_valid(&co_data.uuid);

    if !uuid_valid {
        return RegistrationStatus::InvalidUuid;
    }

    if !endpoint_valid {
        return RegistrationStatus::InvalidEndpoint;
    }

    let co = match DB.get(&co_data.uuid) {
        Ok(co) => co,
        Err(_) => {
            return RegistrationStatus::New;
        }
    };

    if co.device_id == co_data.device_id && co.password == co_data.password {
        // Credentials are not updated
        if co.forbidden {
            RegistrationStatus::Forbidden
        } else if co.endpoint != co_data.endpoint {
            RegistrationStatus::Updated
        } else {
            RegistrationStatus::Running
        }
    } else {
        RegistrationStatus::Updated
    }
}

fn gen_rep(mut map: HashMap<String, String>) -> Json<Response> {
    map.insert(
        String::from("version"),
        env!("CARGO_PKG_VERSION").to_string(),
    );
    Json(Response { mollysocket: map })
}

pub async fn launch() {
    if !config::should_start_webserver() {
        log::warn!("The web server is disabled, making mollysocket run in an air gapped mode. With this clients are less easy to set up and push might break.");
        return;
    }

    let rocket_cfg = rocket::Config::figment()
        .merge(("address", &config::get_host()))
        .merge(("port", &config::get_port()));

    let _ = rocket::build()
        .configure(rocket_cfg)
        .mount("/", routes![discover, register])
        .mount_metrics("/metrics", &METRICS)
        .launch()
        .await;
}
