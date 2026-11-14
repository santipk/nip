# NIP nativo

Implementación Windows x64 en Rust: egui/eframe y AccessKit para la interfaz, CPAL/WASAPI para audio, Opus mono a 48 kHz y QUIC/TLS 1.3 con quinn/rustls. No incorpora Chromium ni WebView.

La rama `codex/native-migration` está en verificación. La distribución pública Electron v0.1.2 sigue disponible; no se ha sustituido por una release nativa sin terminar de verificar.

## Desarrollo sin reinstalar

Requiere Rust mediante rustup, Visual Studio Build Tools con C++/Windows SDK y CMake. `native/rust-toolchain.toml` fija Rust 1.100.0; Cargo.lock fija las dependencias. NSIS sólo se necesita para generar el instalador.

Desde la raíz del repositorio, en dos terminales PowerShell:

```powershell
.\scripts\dev-native.ps1 -Profile anfitrion -Release
.\scripts\dev-native.ps1 -Profile amigo -Release
```

Cada comando compila incrementalmente y abre un perfil independiente en `work/native-<nombre>`. Cerrá y volvé a ejecutar después de editar. No requiere instalación ni utiliza tus datos de Electron. Para compilar más rápido durante desarrollo, omití `-Release`.

## Uso y conectividad

Elegí tu nombre y creá un servidor, o conectate con una invitación. El micrófono permanece apagado hasta entrar a la llamada o iniciar explícitamente la prueba privada. El chat funciona fuera de llamada: texto plano de hasta 2000 caracteres Unicode, cinco mensajes cada diez segundos por participante e historial de los últimos cien mensajes en memoria del anfitrión. El historial desaparece al detener el servidor.

Las invitaciones `nip-native://join#...` se pegan dentro de la app. Contienen un certificado de confianza y un permiso aleatorio de 256 bits; son credenciales compartidas, no identificaciones personales. Cualquiera que reciba el enlace puede reenviarlo. No hay revocación individual ni expiración automática.

El anfitrión debe tener un puerto UDP alcanzable (45454 por defecto). Para dos clientes locales usá `127.0.0.1:45454`; para LAN, la IP local del anfitrión. Se admiten direcciones IP numéricas, sin resolución DNS. NIP no modifica el firewall, el router ni la red. Internet puede requerir redirección de puertos; CGNAT y redes restrictivas pueden impedir la conexión. No hay STUN/TURN, NAT traversal ni relay público. La prueba local no verifica WAN.

El anfitrión retransmite audio Opus a los participantes autenticados. TLS protege cada conexión; **el anfitrión puede escuchar el audio**. No se promete E2EE frente al anfitrión. El protocolo y las invitaciones son incompatibles con Electron v0.1.2: todos los participantes deben usar el cliente nativo.

## Audio y privacidad

Entrada y salida seleccionan los dispositivos de NIP. Nivel del micrófono y volumen de la app ajustan señales dentro de NIP, de 0 a 100%; no cambian los niveles globales de Windows. Las preferencias se guardan localmente. La ayuda de cada fila se abre con hover, foco de teclado o clic; Escape o clic afuera la cierra.

La prueba privada reproduce localmente la señal procesada y suspende el envío antes de habilitar el monitor. Al terminar restaura el mute anterior. Ensordecer detiene la prueba, la captura y la reproducción remota; desactivarlo conserva el mute previo. No se graban archivos de audio. Usá auriculares para evitar realimentación.

El backend requiere un formato de dispositivo compatible con 48 kHz. No incluye remuestreo, cancelación de eco ni supresión de ruido; no muestra controles que finjan aplicar esos efectos. Las pruebas sintéticas no certifican la compatibilidad de un micrófono o parlante físico.

## Instalación y datos

El instalador nativo usa una ubicación y accesos propios: `%LOCALAPPDATA%\Programs\NIPNative`, `NIP Native` en escritorio/Inicio y perfil `%APPDATA%\NIPNative`. No importa, sobrescribe ni borra el perfil Electron. Identidad privada, invitaciones y preferencias se guardan cifradas mediante DPAPI del usuario de Windows; esto no protege frente a malware ejecutado por ese mismo usuario.

La pantalla final ofrece opciones independientes para escritorio, menú Inicio y apertura inmediata. Crear un acceso en Inicio no equivale a anclarlo. La desinstalación conserva los datos. El build `-TestBuild` usa nombres, registro y directorios de prueba distintos.

```powershell
cd native
cargo test --release --locked
cd ..
.\scripts\package-native.ps1 -TestBuild
.\scripts\test-native-installer.ps1 -TestBuild
.\scripts\benchmark-native.ps1
```

La prueba de extremo a extremo conecta dos clientes QUIC reales con fuente y salida de audio sintéticas. Verifica chat sin captura, audio decodificado, monitor sin publicación, entrada/salida de pares, mute/deafen/restauración, nivel, volumen y limpieza. La prueba de instalación ejecuta el binario instalado con un perfil aislado y comprueba retención de datos y conservación del ejecutable Electron existente.

## Medición local

Muestra de 10 segundos por aplicación con perfil nuevo, ventana de inicio, sin servidor ni audio; tres segundos de estabilización. Equipo con seis procesadores lógicos. No es un benchmark de llamadas ni un promedio de múltiples ejecuciones.

| Métrica | Electron v0.1.2 | Rust nativo |
|---|---:|---:|
| Procesos | 6 | 1 |
| Working set total | 586,41 MiB | 141,67 MiB |
| Bytes privados totales | 227,81 MiB | 136,55 MiB |
| CPU sobre un núcleo | 1,525% | 1,070% |
| CPU sobre el equipo | 0,254% | 0,178% |
| Primer handle de ventana | 517 ms | 389 ms |

El working set sumado puede contar páginas compartidas varias veces. Los bytes privados se informan por separado. El arranque mide aparición del handle de ventana, no disponibilidad para interacción ni latencia de audio. Script reproducible: `scripts/benchmark-native.ps1`; resultado local completo: `work/native-benchmark.json`.
