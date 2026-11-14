# Rust nativo y protocolo QUIC

Estado: implementación autorizada por el usuario, en verificación.

La aplicación usa egui/eframe, CPAL/WASAPI y Opus, Tokio/quinn/rustls. No incorpora Chromium ni WebView. Usa bibliotecas existentes para interfaz, codec y criptografía; la distribución conserva sus licencias. Egui renderiza los controles y expone accesibilidad mediante AccessKit. Las mediciones de memoria total, CPU y arranque se realizan sobre procesos controlados; ver [resultados y metodología](../NATIVE.md#medición-local).

El audio es Opus mono a 48 kHz con frames de 20 ms. El anfitrión retransmite los paquetes por QUIC/TLS 1.3. Esto evita implementar ICE/DTLS/SRTP de un cliente WebRTC nativo durante la migración. El anfitrión recibe los paquetes Opus y puede escucharlos: el cifrado es por salto y no ofrece E2EE frente al anfitrión. UDP debe llegar al anfitrión; no se incluyen STUN/TURN, NAT traversal ni relay público. El tráfico de retransmisión crece con el grupo y no se certifica capacidad para salas grandes. Las invitaciones y el protocolo son incompatibles con Electron.

Los certificados se generan con rcgen. El cliente confía en el certificado de la invitación mediante RootCertStore y valida el nombre nip.local; no se desactiva la validación TLS. El permiso aleatorio tiene 256 bits. La autenticación precede al chat y al audio. Identidades y permisos se guardan mediante DPAPI de Windows en un perfil separado. No se importa ni borra el perfil Electron.

La paleta existente es una preferencia explícita del usuario. Impeccable se aplica a jerarquía y espaciado, conservando los colores. Chat, monitor privado, ajustes y barra de audio son funciones reales con pruebas.

Fuentes de bibliotecas y licencias: [egui](https://github.com/emilk/egui), [CPAL](https://github.com/RustAudio/cpal), [opus-rs](https://github.com/SpaceManiac/opus-rs), [quinn](https://github.com/quinn-rs/quinn), [rustls](https://github.com/rustls/rustls). El empaquetado genera avisos de dependencias a partir del lockfile e incluye los textos disponibles en sus fuentes, incluidos libopus y fuentes tipográficas.