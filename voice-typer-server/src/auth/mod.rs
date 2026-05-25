pub mod password;
pub mod routes;
pub mod session;

use rand::Rng;

pub fn new_user_token() -> String {
    let bytes: [u8; 32] = rand::thread_rng().gen();
    base32::encode(base32::Alphabet::Rfc4648 { padding: false }, &bytes)
}

pub fn new_invite_code() -> String {
    let bytes: [u8; 8] = rand::thread_rng().gen();
    base32::encode(base32::Alphabet::Rfc4648 { padding: false }, &bytes)
}

pub fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}
