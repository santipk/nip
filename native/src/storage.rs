use crate::contracts::{AudioSettings, SavedServer};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct Invitation {
    pub address: String,
    pub certificate: Vec<u8>,
    pub token: Vec<u8>,
}
impl Invitation {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.len() > 16384 {
            return Err("Invitación demasiado larga".into());
        }
        let encoded = value
            .strip_prefix("nip-native://join#")
            .ok_or("Invitación inválida")?;
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| "Invitación inválida")?;
        let invite: Self = serde_json::from_slice(&bytes).map_err(|_| "Invitación inválida")?;
        invite.validate()?;
        Ok(invite)
    }
    pub fn validate(&self) -> Result<(), String> {
        let address: SocketAddr = self
            .address
            .parse()
            .map_err(|_| "Usá una dirección IP y puerto válidos")?;
        if address.port() == 0
            || self.token.len() != 32
            || self.certificate.is_empty()
            || self.certificate.len() > 8192
        {
            return Err("Invitación inválida".into());
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<String, String> {
        self.validate()?;
        Ok(format!(
            "nip-native://join#{}",
            URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(self).map_err(|_| "No se pudo crear la invitación")?)
        ))
    }
    pub fn id(&self) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(&self.certificate))
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct HostIdentity {
    pub certificate: Vec<u8>,
    pub key: Vec<u8>,
    pub token: Vec<u8>,
    pub name: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Data {
    host: Option<HostIdentity>,
    servers: Vec<(SavedServer, Invitation)>,
    audio: AudioSettings,
}
pub struct Store {
    path: PathBuf,
    data: Data,
}
impl Store {
    pub fn open(profile: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&profile).map_err(|_| "No se pudo crear el perfil nativo")?;
        let path = profile.join("native-secrets.dpapi");
        let data = if path.exists() {
            serde_json::from_slice(&unprotect(
                &std::fs::read(&path).map_err(|_| "No se pudo leer el perfil")?,
            )?)
            .map_err(|_| "Perfil nativo dañado")?
        } else {
            Data::default()
        };
        Ok(Self { path, data })
    }
    fn save(&self) -> Result<(), String> {
        let raw = serde_json::to_vec(&self.data).map_err(|_| "No se pudo guardar el perfil")?;
        let encrypted = protect(&raw)?;
        let temp = self.path.with_extension("tmp");
        std::fs::write(&temp, encrypted).map_err(|_| "No se pudo guardar el perfil")?;
        std::fs::rename(temp, &self.path).map_err(|_| "No se pudo actualizar el perfil".to_string())
    }
    pub fn host(&mut self, name: &str) -> Result<HostIdentity, String> {
        if self.data.host.is_none() {
            let certified = rcgen::generate_simple_self_signed(vec!["nip.local".into()])
                .map_err(|_| "No se pudo crear el certificado")?;
            let mut token = vec![0; 32];
            getrandom::fill(&mut token).map_err(|_| "No se pudo generar el permiso")?;
            self.data.host = Some(HostIdentity {
                certificate: certified.cert.der().to_vec(),
                key: certified.signing_key.serialize_der(),
                token,
                name: name.into(),
            });
        }
        self.data.host.as_mut().unwrap().name = name.into();
        self.save()?;
        Ok(self.data.host.clone().unwrap())
    }
    pub fn remember(&mut self, name: String, invite: Invitation) -> Result<(), String> {
        let id = invite.id();
        self.data.servers.retain(|(s, _)| s.id != id);
        self.data.servers.push((
            SavedServer {
                id,
                name,
                endpoint: invite.address.clone(),
            },
            invite,
        ));
        self.save()
    }
    pub fn saved(&self) -> Vec<SavedServer> {
        self.data.servers.iter().map(|(s, _)| s.clone()).collect()
    }
    pub fn invitation(&self, id: &str) -> Option<Invitation> {
        self.data
            .servers
            .iter()
            .find(|(s, _)| s.id == id)
            .map(|(_, i)| i.clone())
    }
    pub fn audio(&self) -> AudioSettings {
        self.data.audio.clone()
    }
    pub fn save_audio(&mut self, settings: AudioSettings) -> Result<(), String> {
        self.data.audio = settings;
        self.save()
    }
}
#[cfg(windows)]
fn crypt(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    unsafe {
        let success = if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if success == 0 {
            return Err("Windows no pudo proteger o abrir el perfil DPAPI de este usuario".into());
        }
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());
        Ok(result)
    }
}
#[cfg(not(windows))]
fn crypt(_: &[u8], _: bool) -> Result<Vec<u8>, String> {
    Err("El almacenamiento protegido requiere Windows DPAPI".into())
}
fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    crypt(data, true)
}
fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    crypt(data, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invitation_rejects_invalid_address_and_secret() {
        assert!(Invitation::parse("nip-native://join#e30").is_err());
    }
    #[test]
    #[cfg(windows)]
    fn identity_and_permissions_survive_restart() {
        let path = std::env::temp_dir().join(format!("nip-native-store-{}", std::process::id()));
        let mut store = Store::open(path.clone()).unwrap();
        let first = store.host("Prueba").unwrap();
        drop(store);
        let mut reopened = Store::open(path.clone()).unwrap();
        let second = reopened.host("Prueba").unwrap();
        assert_eq!(first.certificate, second.certificate);
        assert_eq!(first.token, second.token);
        assert!(
            !std::fs::read(path.join("native-secrets.dpapi"))
                .unwrap()
                .windows(32)
                .any(|x| x == first.token)
        );
        std::fs::remove_dir_all(path).unwrap();
    }
}
