use serde::{Deserialize, Serialize};
use url::Url;

pub fn get_media_download_url(mxc_url: String) -> String {
    let url_parts_raw = mxc_url.replace("mxc://", "");
    let url_parts: Vec<&str> = url_parts_raw.split('/').collect();
    let server_name = (*url_parts.first().unwrap()).to_string();
    let media_id = (*url_parts.last().unwrap()).to_string();
    let new_path = format!("_matrix/media/r0/download/{}/{}", server_name, media_id,);
    // FIX this madness
    let mut new_url = Url::parse(format!("https://matrix.{}", server_name).as_str()).unwrap();
    new_url.set_path(new_path.as_str());
    println!("{}", new_url);
    new_url.to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    /// The access token used for this session.
    pub access_token: String,
    /// The user the access token was issued for.
    pub user_id: String,
    /// The ID of the client device
    pub device_id: String,
}
