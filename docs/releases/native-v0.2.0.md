# NIP Native 0.2.0 — Windows x64

Cliente completo en Rust nativo, sin Chromium ni WebView: servidores autohospedados, invitaciones privadas, audio Opus, chat sin entrar a una llamada, ajustes de dispositivos/niveles/volumen y monitor privado. Conserva la paleta de NIP y agrega ayudas de teclado y accesibilidad nativa.

El instalador `NIP-Native-Setup-0.2.0-x64.exe` se instala por usuario como **NIP Native**, junto a la versión Electron. Ofrece opciones independientes para escritorio, menú Inicio y apertura inmediata. No importa ni borra datos anteriores. La desinstalación conserva el perfil.

Se verificaron 26 pruebas automatizadas, dos clientes QUIC con audio sintético, ejecución del binario instalado, retención de datos y las ocho combinaciones de la pantalla final. La compatibilidad con micrófonos/parlantes físicos y redes externas debe comprobarse con los equipos del grupo. Es la primera distribución nativa, publicada como prerelease.

## Compatibilidad y límites

- Protocolo QUIC/TLS 1.3 nuevo: las invitaciones Electron no funcionan. Todos los participantes deben usar el cliente nativo. Requiere un puerto UDP alcanzable en el anfitrión; 45454 por defecto.
- El anfitrión retransmite audio y puede escucharlo. Cifrado por salto; no E2EE frente al anfitrión.
- Direcciones IP numéricas; sin DNS, STUN/TURN, NAT traversal ni relay público. No modifica firewall ni router.
- Audio mono a 48 kHz. No incluye remuestreo, cancelación de eco ni supresión de ruido. Usá auriculares en la prueba privada.
- Chat: 2000 caracteres Unicode por mensaje, cinco mensajes cada diez segundos por participante y cien mensajes en RAM; se pierde al detener el anfitrión.
- Sin firma Authenticode. SignPath confirmó recepción de la solicitud; la aprobación y el certificado están pendientes. Incluye checksum SHA-256 y avisos de licencias.

Para ejecutar incrementalmente desde el repositorio sin instalar: `.\scripts\dev-native.ps1 -Profile dev -Release`. Más detalles en [la documentación nativa](https://github.com/santipk/nip/blob/codex/native-migration/docs/NATIVE.md).
