# NIP

**name in progress**. Aplicación de escritorio para llamadas y chat de amigos, con servidores que cada grupo aloja en su equipo. El cliente Windows x64 está implementado en **Rust nativo, sin Chromium ni WebView**.

## Windows

[Descargar NIP Native 0.2.0](https://github.com/santipk/nip/releases/tag/native-v0.2.0). El instalador se llama `NIP-Native-Setup-0.2.0-x64.exe`. Es una primera distribución nativa; la compatibilidad con audio físico y redes externas debe probarse con los equipos del grupo. Incluye checksum SHA-256 y avisos de dependencias.

Se instala por usuario, con accesos **NIP Native**, en una ubicación distinta de Electron. No reemplaza tu instalación anterior ni importa o borra sus datos. No requiere Rust, Node.js ni permisos de administrador para usarla. La distribución está **sin firma Authenticode**; la solicitud a SignPath Foundation fue recibida, con aprobación pendiente. Ver [política de firma y privacidad](CODE_SIGNING.md).

## Desarrollo sin reinstalar

Requiere Rust mediante rustup, MSVC C++ Build Tools/Windows SDK y CMake. La versión de Rust y las dependencias están fijadas. Desde la raíz del repositorio:

```powershell
.\scripts\dev-native.ps1 -Profile anfitrion -Release
```

Para otro cliente, en otra terminal:

```powershell
.\scripts\dev-native.ps1 -Profile amigo -Release
```

Cada comando compila incrementalmente y abre un perfil aislado en `work/`. Omití `-Release` para desarrollo. No instala la aplicación.

## Funciones

- Servidores autohospedados con identidad estable e invitaciones privadas; secretos cifrados con DPAPI de Windows.
- Chat sin entrar a una llamada: hasta 2000 caracteres Unicode por mensaje, cinco mensajes cada diez segundos por participante y los últimos cien mensajes en RAM del anfitrión.
- Audio Opus mono a 48 kHz, mute, deafen y prueba privada con suspensión del envío al grupo.
- Entrada/salida, nivel de micrófono y volumen dentro de NIP; preferencias locales y ayudas accesibles en cada fila.
- Instalador con opciones independientes para escritorio, Inicio y apertura inmediata. La desinstalación conserva los datos.

El protocolo nativo usa QUIC/TLS 1.3 y necesita UDP alcanzable al anfitrión (45454 por defecto). No es compatible con las invitaciones Electron. No incluye DNS, NAT traversal, STUN/TURN ni relay público. El anfitrión retransmite audio y puede escucharlo: **no ofrece E2EE frente al anfitrión**. No incluye cancelación de eco, supresión de ruido ni remuestreo; los dispositivos deben admitir un formato de 48 kHz. Usá auriculares para la prueba privada.

Ver [uso, seguridad, límites y mediciones reales](docs/NATIVE.md), [decisión de arquitectura](docs/adr/0001-native-rust.md) y [transición](docs/TRANSITION.md).

## Verificar y empaquetar

```powershell
cd native
cargo test --release --locked
cd ..
.\scripts\package-native.ps1 -TestBuild
.\scripts\test-native-installer.ps1 -TestBuild
```

Las pruebas conectan dos clientes QUIC reales con audio sintético y comprueban chat, autenticación, límites, historial máximo, señal decodificada, monitor sin publicación, restauración de mute/deafen, nivel/volumen y controles de privacidad durante conexiones lentas. La prueba de instalación abre el ejecutable instalado con un perfil independiente y comprueba conservación de datos. No certifican hardware físico ni WAN.

La implementación anterior se conserva en `src/` y tiene [documentación histórica](docs/ELECTRON.md). El empaquetado nativo usa sólo `native/` y sus dependencias Rust; no incorpora Electron.

Licencia [MIT](LICENSE). Las dependencias conservan sus licencias y avisos en la distribución.