use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use md5::Md5;
use mlua::{Lua, LuaString, Table, Value};
use sha2::{Digest, Sha256};

/// `wayrun.crypto`: the signing and encoding primitives API clients keep
/// needing, so a script does not ship its own sha256 or base64.
pub(super) fn lib(lua: &Lua) -> mlua::Result<Table> {
    let crypto = lua.create_table()?;
    crypto.set(
        "sha256",
        lua.create_function(|_, text: LuaString| Ok(hex(&Sha256::digest(text.as_bytes()))))?,
    )?;
    crypto.set(
        "md5",
        lua.create_function(|_, text: LuaString| Ok(hex(&Md5::digest(text.as_bytes()))))?,
    )?;
    crypto.set(
        "hmac_sha256",
        lua.create_function(|_, (key, text): (LuaString, LuaString)| {
            let mut mac = Hmac::<Sha256>::new_from_slice(&key.as_bytes())
                .expect("hmac takes a key of any length");
            mac.update(&text.as_bytes());
            Ok(hex(&mac.finalize().into_bytes()))
        })?,
    )?;
    crypto.set(
        "base64_encode",
        lua.create_function(|_, text: LuaString| {
            Ok(base64::engine::general_purpose::STANDARD.encode(text.as_bytes()))
        })?,
    )?;
    crypto.set(
        "base64_decode",
        lua.create_function(
            |lua, text: LuaString| match base64::engine::general_purpose::STANDARD
                .decode(text.as_bytes())
            {
                Ok(bytes) => Ok(Value::String(lua.create_string(bytes)?)),
                Err(_) => Ok(Value::Nil),
            },
        )?,
    )?;
    Ok(crypto)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = Lua::new();
        lua.globals().set("crypto", lib(&lua).unwrap()).unwrap();
        lua
    }

    #[test]
    fn hashes_match_their_known_vectors() {
        let lua = lua();
        let (sha, md5, hmac): (String, String, String) = lua
            .load(
                r#"
                return crypto.sha256("abc"), crypto.md5("abc"),
                       crypto.hmac_sha256("Jefe", "what do ya want for nothing?")
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(
            sha,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(md5, "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hmac,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn base64_round_trips_bytes_and_refuses_garbage() {
        let lua = lua();
        let (encoded, decoded, refused): (String, bool, bool) = lua
            .load(
                r#"
                local encoded = crypto.base64_encode("\0\255")
                local decoded = crypto.base64_decode(encoded)
                return encoded, decoded == "\0\255", crypto.base64_decode("not base64!") == nil
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(encoded, "AP8=");
        assert!(decoded);
        assert!(refused);
    }
}
