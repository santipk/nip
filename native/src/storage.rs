use crate::contracts::{AudioSettings, LocalServerSettings, Preferences, SavedServer};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct Invitation {
    #[serde(default)]
    pub relay: Option<crate::relay::RelayEndpoint>,
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
        if let Some(relay) = &self.relay {
            relay.validate()?;
        }
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
impl HostIdentity {
    pub fn id(&self) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(&self.certificate))
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct LocalServer {
    #[serde(default)]
    pub relay: Option<crate::contracts::RelayConfiguration>,
    pub identity: HostIdentity,
    pub settings: LocalServerSettings,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Data {
    #[serde(default)]
    display_name: String,
    host: Option<HostIdentity>,
    #[serde(default)]
    hosts: Vec<LocalServer>,
    servers: Vec<(SavedServer, Invitation)>,
    audio: AudioSettings,
    #[serde(default)]
    preferences: Preferences,
}
#[derive(Clone)]
pub struct Store {
    path: PathBuf,
    data: Data,
}
impl Store {
    pub fn configure_relay(
        &mut self,
        id: &str,
        relay: Option<crate::contracts::RelayConfiguration>,
    ) -> Result<(), String> {
        validate_relay(relay.as_ref())?;
        let previous = self.data.clone();
        let server = self
            .data
            .hosts
            .iter_mut()
            .find(|s| s.identity.id() == id)
            .ok_or("El servidor no pertenece a este perfil")?;
        server.relay = relay;
        if let Err(error) = self.save() {
            self.data = previous;
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn portable_catalog(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&self.data).map_err(|_| "No se pudo exportar el catálogo".into())
    }
    pub(crate) fn restore_catalog(profile: PathBuf, bytes: &[u8]) -> Result<Self, String> {
        let mut data: Data =
            serde_json::from_slice(bytes).map_err(|_| "Catálogo de respaldo inválido")?;
        if data.host.is_some() || data.hosts.len() > 1024 || data.servers.len() > 4096 {
            return Err("Catálogo de respaldo incompatible o demasiado grande".into());
        }
        crate::audio::validate(&data.audio)?;
        if data.display_name.chars().count() > 64
            || data.preferences.port == 0
            || !(1..=32).contains(&data.preferences.max_participants)
        {
            return Err("Preferencias de respaldo inválidas".into());
        }
        let mut ids = std::collections::HashSet::new();
        for server in &mut data.hosts {
            Self::validate_server(&server.settings)?;
            validate_relay(server.relay.as_ref())?;
            let identity = &server.identity;
            if identity.token.len() != 32
                || identity.name.trim().is_empty()
                || identity.name.chars().count() > 64
                || !ids.insert(identity.id())
            {
                return Err("Identidad de respaldo inválida o repetida".into());
            }
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![rustls::pki_types::CertificateDer::from(
                        identity.certificate.clone(),
                    )],
                    rustls::pki_types::PrivatePkcs8KeyDer::from(identity.key.clone()).into(),
                )
                .map_err(|_| "Certificado y clave de respaldo inválidos")?;
            server.settings.auto_start = false;
        }
        let mut saved_ids = std::collections::HashSet::new();
        for (saved, invitation) in &data.servers {
            invitation.validate()?;
            if saved.id != invitation.id() || !saved_ids.insert(saved.id.clone()) {
                return Err("Invitación de respaldo inválida o repetida".into());
            }
        }
        data.preferences.auto_reconnect = false;
        data.preferences.start_with_windows = false;
        let store = Self {
            path: profile.join("native-secrets.dpapi"),
            data,
        };
        if store.path.exists() {
            return Err("El perfil de destino ya contiene datos".into());
        }
        store.save()?;
        Ok(store)
    }
    pub fn local_servers(&self) -> Vec<LocalServer> {
        self.data.hosts.clone()
    }
    pub fn local_server(&self, id: &str) -> Option<LocalServer> {
        self.data
            .hosts
            .iter()
            .find(|s| s.identity.id() == id)
            .cloned()
    }
    pub fn history_path(&self, id: &str) -> Result<PathBuf, String> {
        if !self.owns(id) {
            return Err("El servidor no pertenece a este perfil.".into());
        }
        Ok(self
            .path
            .parent()
            .ok_or("Perfil sin carpeta")?
            .join("servers")
            .join(id)
            .join("history.sqlite3"))
    }
    pub fn configure_server(
        &mut self,
        id: &str,
        settings: LocalServerSettings,
    ) -> Result<(), String> {
        Self::validate_server(&settings)?;
        let index = self
            .data
            .hosts
            .iter()
            .position(|s| s.identity.id() == id)
            .ok_or("El servidor no pertenece a este perfil.")?;
        let previous = self.data.clone();
        self.data.hosts[index].settings = settings.clone();
        if let Some((server, invite)) = self.data.servers.iter_mut().find(|(s, _)| s.id == id) {
            invite.address = format!("127.0.0.1:{}", settings.port);
            server.endpoint = invite.address.clone();
        }
        if let Err(error) = self.save() {
            self.data = previous;
            return Err(error);
        }
        Ok(())
    }
    fn validate_server(settings: &LocalServerSettings) -> Result<(), String> {
        if settings.file_limit_bytes == 0
            || settings.storage_limit_bytes == 0
            || settings.file_limit_bytes > settings.storage_limit_bytes
            || settings.storage_limit_bytes > i64::MAX as u64
        {
            return Err("Revisá los límites: archivo y servidor positivos, archivo no mayor al servidor y cuota representable en bytes".into());
        }
        if settings.port == 0
            || !(1..=32).contains(&settings.max_participants)
            || (settings.auto_start && !settings.persistent)
        {
            return Err("Revisá puerto (1–65535), participantes (1–32) e inicio automático (sólo persistentes).".into());
        }
        Ok(())
    }
    pub fn create_server(
        &mut self,
        name: &str,
        settings: LocalServerSettings,
    ) -> Result<LocalServer, String> {
        Self::validate_server(&settings)?;
        if name.trim().is_empty() || name.chars().count() > 64 {
            return Err("El nombre debe contener entre 1 y 64 caracteres.".into());
        }
        let certified = rcgen::generate_simple_self_signed(vec!["nip.local".into()])
            .map_err(|_| "No se pudo crear el certificado")?;
        let mut token = vec![0; 32];
        getrandom::fill(&mut token).map_err(|_| "No se pudo generar el permiso")?;
        let server = LocalServer {
            relay: None,
            identity: HostIdentity {
                certificate: certified.cert.der().to_vec(),
                key: certified.signing_key.serialize_der(),
                token,
                name: name.trim().into(),
            },
            settings,
        };
        let previous = self.data.clone();
        self.data.hosts.push(server.clone());
        let invite = Invitation {
            relay: None,
            address: format!("127.0.0.1:{}", server.settings.port),
            certificate: server.identity.certificate.clone(),
            token: server.identity.token.clone(),
        };
        if let Err(error) = self.remember(server.identity.name.clone(), invite) {
            self.data = previous;
            return Err(error);
        }
        Ok(server)
    }
    pub fn preferences(&self) -> Preferences {
        self.data.preferences.clone()
    }
    pub fn save_preferences(&mut self, preferences: Preferences) -> Result<(), String> {
        if preferences.port == 0 || !(1..=32).contains(&preferences.max_participants) {
            return Err("Puerto válido: 1–65535. Participantes: 1–32.".into());
        }
        for key in [preferences.mute_shortcut, preferences.deafen_shortcut] {
            if key != 0 && !crate::audio::valid_hotkey(key) {
                return Err("Atajo no admitido.".into());
            }
        }
        if preferences.mute_shortcut != 0
            && preferences.mute_shortcut == preferences.deafen_shortcut
        {
            return Err("Elegí atajos diferentes para silenciar y ensordecer.".into());
        }
        let startup_changed =
            preferences.start_with_windows != self.data.preferences.start_with_windows;
        if startup_changed {
            crate::desktop::configure_startup(
                preferences.start_with_windows,
                self.path.parent().ok_or("Perfil sin carpeta.")?,
            )?;
        }
        let previous = std::mem::replace(&mut self.data.preferences, preferences);
        if let Err(error) = self.save() {
            if startup_changed {
                let _ = crate::desktop::configure_startup(
                    previous.start_with_windows,
                    self.path.parent().unwrap(),
                );
            }
            self.data.preferences = previous;
            return Err(error);
        }
        Ok(())
    }
    pub fn regenerate_invitation(&mut self, id: &str) -> Result<HostIdentity, String> {
        let previous = self.data.clone();
        let mut token = vec![0; 32];
        getrandom::fill(&mut token).map_err(|_| "No se pudo generar una nueva invitación.")?;
        let host = self
            .data
            .hosts
            .iter_mut()
            .find(|s| s.identity.id() == id)
            .map(|s| &mut s.identity)
            .ok_or("No hay un servidor propio guardado.")?;
        host.token = token;
        let identity = host.clone();
        for (_, invite) in &mut self.data.servers {
            if invite.certificate == identity.certificate {
                invite.token = identity.token.clone();
            }
        }
        if let Err(error) = self.save() {
            self.data = previous;
            return Err(error);
        }
        Ok(identity)
    }
    pub fn display_name(&self) -> String {
        self.data.display_name.clone()
    }
    pub fn save_display_name(&mut self, name: String) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err("El nombre debe contener entre 1 y 64 caracteres.".into());
        }
        let previous = std::mem::replace(&mut self.data.display_name, name.into());
        if let Err(error) = self.save() {
            self.data.display_name = previous;
            return Err(error);
        }
        Ok(())
    }
    pub fn open(profile: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&profile).map_err(|_| "No se pudo crear el perfil nativo")?;
        let path = profile.join("native-secrets.dpapi");
        let mut data: Data = if path.exists() {
            serde_json::from_slice(&unprotect(
                &std::fs::read(&path).map_err(|_| "No se pudo leer el perfil")?,
            )?)
            .map_err(|_| "Perfil nativo dañado")?
        } else {
            Data::default()
        };
        // Legacy profiles had a single host identity. Migrate it without changing invitations.
        if let Some(identity) = data.host.take() {
            if !data.hosts.iter().any(|s| s.identity.id() == identity.id()) {
                let backup = path.with_extension("legacy.dpapi");
                if !backup.exists() {
                    std::fs::copy(&path, &backup).map_err(|e| {
                        format!("No se pudo respaldar el perfil anterior antes de migrarlo: {e}")
                    })?;
                }
                let port = data
                    .servers
                    .iter()
                    .find(|(s, _)| s.id == identity.id())
                    .and_then(|(_, i)| i.address.parse::<SocketAddr>().ok())
                    .map_or(data.preferences.port, |a| a.port());
                data.hosts.push(LocalServer {
                    relay: None,
                    identity,
                    settings: LocalServerSettings {
                        port,
                        max_participants: data.preferences.max_participants,
                        ..Default::default()
                    },
                });
            }
        }
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
        if let Some(server) = self.data.hosts.iter().find(|s| s.identity.name == name) {
            return Ok(server.identity.clone());
        }
        Ok(self
            .create_server(name, LocalServerSettings::default())?
            .identity)
    }
    pub fn remember(&mut self, name: String, invite: Invitation) -> Result<(), String> {
        let id = invite.id();
        let alias = self
            .data
            .servers
            .iter()
            .find(|(s, _)| s.id == id)
            .and_then(|(s, _)| s.alias.clone());
        self.data.servers.retain(|(s, _)| s.id != id);
        self.data.servers.push((
            SavedServer {
                id,
                name,
                endpoint: invite.address.clone(),
                alias,
                owned: false,
                local: None,
            },
            invite,
        ));
        self.save()
    }
    pub fn saved(&self) -> Vec<SavedServer> {
        let mut saved: Vec<_> = self
            .data
            .servers
            .iter()
            .map(|(s, _)| {
                let mut s = s.clone();
                s.owned = self.owns(&s.id);
                s.local = self.local_server(&s.id).map(|s| s.settings);
                s
            })
            .collect();
        for server in &self.data.hosts {
            let id = server.identity.id();
            if !saved.iter().any(|s| s.id == id) {
                saved.push(SavedServer {
                    id,
                    name: server.identity.name.clone(),
                    endpoint: format!("127.0.0.1:{}", server.settings.port),
                    alias: None,
                    owned: true,
                    local: Some(server.settings.clone()),
                });
            }
        }
        saved
    }
    pub(crate) fn owns(&self, id: &str) -> bool {
        self.data.hosts.iter().any(|s| s.identity.id() == id)
    }
    pub fn rename_server(&mut self, id: &str, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err("El nombre debe contener entre 1 y 64 caracteres.".into());
        }
        let owned = self.owns(id);
        if !owned && !self.data.servers.iter().any(|(s, _)| s.id == id) {
            return Err("El servidor ya no está guardado.".into());
        }
        let previous = self.data.clone();
        if let Some((server, _)) = self.data.servers.iter_mut().find(|(s, _)| s.id == id) {
            if owned {
                server.name = name.into();
                server.alias = None;
            } else {
                server.alias = Some(name.into());
            }
        }
        if owned {
            self.data
                .hosts
                .iter_mut()
                .find(|s| s.identity.id() == id)
                .unwrap()
                .identity
                .name = name.into();
        }
        if let Err(error) = self.save() {
            self.data = previous;
            return Err(error);
        }
        Ok(())
    }
    pub fn delete_server(&mut self, id: &str) -> Result<(), String> {
        if !self.owns(id) && !self.data.servers.iter().any(|(s, _)| s.id == id) {
            return Err("El servidor ya no está guardado.".into());
        }
        let previous = self.data.clone();
        if self.owns(id) {
            self.remove_history(id)?;
            self.data.hosts.retain(|s| s.identity.id() != id);
        }
        self.data.servers.retain(|(s, _)| s.id != id);
        if let Err(error) = self.save() {
            self.data = previous;
            return Err(error);
        }
        Ok(())
    }
    fn remove_history(&self, id: &str) -> Result<(), String> {
        let path = self.history_path(id)?;
        for suffix in ["", "-wal", "-shm"] {
            let file = PathBuf::from(format!("{}{suffix}", path.display()));
            match std::fs::remove_file(file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("No se pudo borrar el historial local: {e}")),
            }
        }
        if let Some(directory) = path.parent() {
            let _ = std::fs::remove_dir(directory);
        }
        Ok(())
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
        let previous = std::mem::replace(&mut self.data.audio, settings);
        if let Err(error) = self.save() {
            self.data.audio = previous;
            return Err(error);
        }
        Ok(())
    }
}
fn validate_relay(relay: Option<&crate::contracts::RelayConfiguration>) -> Result<(), String> {
    if let Some(config) = relay {
        config.host.validate()?;
        config.client.validate()?;
        if !(1..=32).contains(&config.tunnels)
            || config.host.route != config.client.route
            || config.host.address != config.client.address
            || config.host.certificate != config.client.certificate
            || config.host.token == config.client.token
        {
            return Err("Configuración relay inválida: revisá ruta, certificado, tokens distintos y túneles (1–32)".into());
        }
    }
    Ok(())
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
pub(crate) fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    crypt(data, true)
}
pub(crate) fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    crypt(data, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn legacy_host_migrates_without_changing_identity_and_new_servers_are_distinct() {
        let directory = std::env::temp_dir().join(format!(
            "nip-catalog-{}-{}",
            std::process::id(),
            getrandom::u64().unwrap()
        ));
        let mut store = Store::open(directory.clone()).unwrap();
        let identity = store.host("Anterior").unwrap();
        store.data.hosts.clear();
        store.data.host = Some(identity.clone());
        store.save().unwrap();
        drop(store);
        let mut store = Store::open(directory.clone()).unwrap();
        assert_eq!(
            store.local_server(&identity.id()).unwrap().identity.token,
            identity.token
        );
        assert!(directory.join("native-secrets.legacy.dpapi").exists());
        let second = store
            .create_server(
                "Segundo",
                LocalServerSettings {
                    persistent: true,
                    auto_start: true,
                    port: 45455,
                    max_participants: 5,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_ne!(identity.id(), second.identity.id());
        assert_eq!(store.saved().len(), 2);
        store
            .rename_server(&second.identity.id(), "Segundo renombrado")
            .unwrap();
        drop(store);
        let store = Store::open(directory.clone()).unwrap();
        assert_eq!(store.local_servers().len(), 2);
        assert_eq!(
            store.local_server(&identity.id()).unwrap().identity.name,
            "Anterior"
        );
        assert_eq!(
            store
                .local_server(&second.identity.id())
                .unwrap()
                .identity
                .name,
            "Segundo renombrado"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    #[cfg(windows)]
    fn server_rename_alias_persistence_and_identity_deletion() {
        let path = std::env::temp_dir().join(format!("nip-server-actions-{}", std::process::id()));
        let mut store = Store::open(path.clone()).unwrap();
        let host = store.host("Own").unwrap();
        let own = Invitation {
            relay: None,
            address: "127.0.0.1:45454".into(),
            certificate: host.certificate.clone(),
            token: host.token.clone(),
        };
        store.remember("Own".into(), own.clone()).unwrap();
        assert!(store.saved()[0].owned);
        store.rename_server(&own.id(), "New own name").unwrap();
        assert_eq!(store.data.hosts[0].identity.name, "New own name");
        let cert = rcgen::generate_simple_self_signed(vec!["nip.local".into()]).unwrap();
        let remote = Invitation {
            relay: None,
            address: "127.0.0.1:45455".into(),
            certificate: cert.cert.der().to_vec(),
            token: vec![42; 32],
        };
        store.remember("Remote".into(), remote.clone()).unwrap();
        store.rename_server(&remote.id(), "My alias").unwrap();
        store
            .remember("Renamed by remote host".into(), remote.clone())
            .unwrap();
        drop(store);
        let mut store = Store::open(path.clone()).unwrap();
        let saved = store.saved();
        let remote_saved = saved.iter().find(|s| s.id == remote.id()).unwrap();
        assert_eq!(remote_saved.name, "Renamed by remote host");
        assert_eq!(remote_saved.display_name(), "My alias");
        assert!(!remote_saved.owned);
        store.delete_server(&remote.id()).unwrap();
        assert!(!store.data.hosts.is_empty());
        store.delete_server(&own.id()).unwrap();
        assert!(store.saved().is_empty() && store.data.hosts.is_empty());
        assert!(store.invitation(&own.id()).is_none());
        assert_ne!(
            store.host("Recreated").unwrap().certificate,
            host.certificate
        );
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn legacy_profile_without_default_name_is_still_readable() {
        let data: Data = serde_json::from_str(r#"{"host":null,"servers":[],"audio":{"input":"","output":"","input_level":1.0,"volume":1.0}}"#).unwrap();
        assert!(data.display_name.is_empty());
        assert_eq!(data.audio.advanced, Default::default());
    }
    #[test]
    #[cfg(windows)]
    fn default_name_is_validated_and_persisted() {
        let path = std::env::temp_dir().join(format!("nip-profile-name-{}", std::process::id()));
        let mut store = Store::open(path.clone()).unwrap();
        assert!(store.save_display_name(" ".into()).is_err());
        assert!(store.save_display_name("ñ".repeat(65)).is_err());
        store.save_display_name("  Santiago ñ  ".into()).unwrap();
        drop(store);
        assert_eq!(
            Store::open(path.clone()).unwrap().display_name(),
            "Santiago ñ"
        );
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    #[cfg(windows)]
    fn processing_preferences_and_invitation_rotation_survive_restart() {
        let path =
            std::env::temp_dir().join(format!("nip-expanded-settings-{}", std::process::id()));
        let mut store = Store::open(path.clone()).unwrap();
        let host = store.host("Ajustes").unwrap();
        let invite = Invitation {
            relay: None,
            address: "127.0.0.1:45454".into(),
            certificate: host.certificate.clone(),
            token: host.token.clone(),
        };
        store.remember("Ajustes".into(), invite.clone()).unwrap();
        let mut audio = AudioSettings::default();
        audio.advanced.microphone_eq.enabled = true;
        audio.advanced.microphone_eq.bands_db[5] = 4.0;
        audio.advanced.noise_suppression = 3;
        store.save_audio(audio.clone()).unwrap();
        let mut preferences = Preferences::default();
        preferences.port = 45455;
        preferences.max_participants = 4;
        preferences.mute_shortcut = 0x78;
        store.save_preferences(preferences.clone()).unwrap();
        let rotated = store.regenerate_invitation(&host.id()).unwrap();
        assert_ne!(rotated.token, host.token);
        assert_eq!(rotated.certificate, host.certificate);
        drop(store);
        let store = Store::open(path.clone()).unwrap();
        assert_eq!(store.audio(), audio);
        assert_eq!(store.preferences(), preferences);
        assert_eq!(store.invitation(&invite.id()).unwrap().token, rotated.token);
        std::fs::remove_dir_all(path).unwrap();
    }
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
