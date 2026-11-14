# NIP

**name in progress**. Aplicación de escritorio para llamadas de amigos, con servidores que cada grupo aloja en su propio equipo. MVP para Windows x64: una sala de audio, invitaciones privadas y servidores recordados localmente.

**Migración nativa en verificación:** la nueva implementación Rust funciona sin Chromium ni WebView e incorpora chat, ajustes de audio y prueba privada. Ver [ejecución nativa, protocolo y mediciones](docs/NATIVE.md). La descarga pública de abajo sigue siendo Electron v0.1.2 hasta terminar las verificaciones del instalador nativo.

## Descargar para Windows

[Descargar el instalador Windows x64](https://github.com/santipk/nip/releases/latest). En Assets elegí `NIP-Setup-<version>-x64.exe`. No requiere Node.js ni permisos de administrador: ejecutá el instalador y abrí NIP desde Inicio o el escritorio. Para desinstalar: Configuración > Aplicaciones > NIP; los datos locales se conservan.

**La primera distribución está sin firma Authenticode.** Windows/SmartScreen o el antivirus pueden advertirlo. SignPath Foundation es la opción elegida para solicitar firma; no hay aprobación ni certificado activo. Ver [Code signing policy](CODE_SIGNING.md). Cada release incluye un checksum SHA-256 para verificar integridad.

## Ejecutar desde el código

Requiere Node.js **22.12 o superior** y npm. Desde PowerShell:

```powershell
git clone https://github.com/santipk/nip.git
cd nip
npm ci
npm start
```

`npm ci` descarga dependencias y Electron; `npm start` compila TypeScript y abre NIP. No instala herramientas globales ni modifica el firewall/router.

Para abrir otra instancia con datos independientes:

```powershell
$env:NIP_DATA_DIR = "$PWD\work\cliente-2"
npm start
```

Usá otra terminal para cada instancia. `NIP_DATA_DIR` cambia sólo el directorio de datos de ese proceso; quitá la variable al volver al perfil normal (`Remove-Item Env:NIP_DATA_DIR`). Sin esa variable, se usa el directorio `userData` de Electron para NIP en el perfil del usuario.

## Uso

1. Elegí tu nombre local y **Crear servidor**. Indicá el nombre del grupo; el acceso es sólo por invitación. El puerto TCP predeterminado es **45454**, configurable en Opciones de red. El micrófono permanece apagado.
2. **Invitar amigos**: elegí una dirección alcanzable por ellos y generá el enlace. `127.0.0.1` sirve sólo para probar dos instancias en la misma computadora. En LAN usá una IP de la interfaz local, por ejemplo `wss://192.168.1.10:45454`.
3. En el otro cliente, **Conectar a servidor** abre un modal para pegar el enlace. El servidor se recuerda sólo después de una conexión autenticada válida.
4. Cada participante debe pulsar **Unirse a la llamada**. Se habilitan silencio, cambio de entrada/salida y salida de llamada. Salir de la llamada conserva el servidor guardado. No se solicita cámara.
5. **Volver a servidores** desconecta este cliente, conservando el proceso anfitrión. **Detener servidor** o cerrar la aplicación del anfitrión termina las conexiones de todos.

La identidad y el permiso del anfitrión se conservan entre reinicios. Para volver a alojar el grupo, pulsá Crear servidor; reutiliza esa identidad. Este MVP administra **un único servidor alojado por perfil local**; no hay arranque automático en segundo plano. Para volver desde otra dirección, recibí una invitación nueva del mismo anfitrión: al conectar válidamente se actualiza el acceso guardado por huella de identidad.

## Red: qué está comprobado y qué requiere configuración

El servidor escucha en todas las interfaces **IPv4** del equipo. Una conexión local exitosa demuestra que el proceso funciona; **no comprueba accesibilidad desde internet**. NIP muestra esta diferencia explícitamente.

Para LAN, ambos clientes necesitan poder comunicarse entre sí. El servidor necesita su puerto TCP entrante; los clientes usan puertos UDP dinámicos de WebRTC para medios directos. Windows puede solicitar autorización de red: decidí vos qué redes permitir. NIP no toca esas reglas.

Para usar internet, el anfitrión administra IP pública o DNS, redirección del puerto TCP hacia su equipo y firewall. Si cambia la IP, puede usar DNS dinámico o distribuir un enlace actualizado. Con doble NAT o **CGNAT**, abrir un puerto en el router puede no ser suficiente; consultá a tu proveedor o usá una red privada administrada por el grupo.

**Abrir el puerto del servidor no resuelve por sí solo el audio.** Los clientes intercambian candidatos ICE locales y deben tener una ruta directa entre sí. No hay servidores STUN/TURN públicos por defecto, ni relay incluido, ni integración de redes virtuales. Esta versión usa `iceServers: []`; no ofrece una pantalla para configurar ICE externos. Redes NAT restrictivas pueden impedir el audio aunque se pueda entrar a la sala. La base verificable es local/LAN; WAN debe probarse con los equipos y redes reales del grupo, y puede requerir una red privada ya existente u otro desarrollo de conectividad.

Una prueba desde otra red requiere a alguien o un mecanismo externo explícito. No se aprovisiona infraestructura para simular esa verificación.

## Seguridad y datos

- La señalización usa **WSS/TLS**, con certificado autofirmado generado en el anfitrión. Cada invitación lleva la huella SHA-256 del certificado. El cliente comprueba esa huella **antes del upgrade WebSocket y antes de enviar el permiso**; un certificado distinto se rechaza. No se desactiva la validación TLS global de Electron.
- El enlace `nip://join#...` contiene dirección, huella y un permiso aleatorio de 256 bits. Se pega dentro de la app; este MVP no registra un manejador de enlaces del sistema. El permiso viaja en el primer mensaje del canal TLS, **no en parámetros de la URL de red ni en logs de la aplicación**.
- El permiso es una credencial compartida del grupo: quien tenga el enlace puede entrar y reenviarlo. Autentica la posesión del permiso, **no la identidad personal ni el nombre visible**. No hay cuentas, invitaciones individuales, expiración ni revocación en esta versión. Si el permiso se filtra, se necesita renovar la identidad/acceso en una versión futura o empezar con un perfil anfitrión nuevo.
- Las claves privadas nunca se incluyen en invitaciones. Identidad y accesos guardados se cifran con `safeStorage` asíncrono de Electron; en Windows usa protección del sistema asociada al usuario. El nombre visible se guarda localmente. Esto no protege frente a software malicioso ejecutado como el mismo usuario. Si falla el almacén seguro, NIP no guarda secretos en texto plano.
- Audio **WebRTC directo** entre participantes, cifrado en transporte mediante DTLS-SRTP. El servidor coordina la sala y retransmite SDP/ICE, no recibe el flujo de audio. La identidad de los pares se negocia mediante el servidor de señalización: **no se promete E2EE verificable frente a un anfitrión malicioso**, ni hay verificación independiente de claves entre amigos. Los participantes conocen información de red de sus pares; el anfitrión procesa IP, nombres, presencia y señalización.
- NIP no implementa telemetría, analítica, cuentas centrales, grabación ni servicio de llamadas del autor. Las descargas y servicios que el grupo utilice pueden tener sus propios metadatos; el carácter abierto del código no elimina obligaciones legales.
- Renderizador aislado y sandboxed, sin Node, contenido local, CSP restrictiva y puente IPC limitado. Se rechazan navegación y ventanas externas. Mensajes de red con tamaño acotado, autenticación con tiempo límite, límite de conexiones y heartbeat. No constituye una auditoría de seguridad ni protección completa frente a DoS en internet.

## Compilar y verificar

```powershell
npm run typecheck
npm test
npm run test:e2e
npm run package
npm run test:package
npm run dist:win
npm run test:installer
```

- `test`: pruebas del servidor real WSS, invitaciones inválidas, permiso incorrecto, pin incorrecto, participantes, enrutamiento autorizado y mensajes malformados.
- `test:e2e`: abre dos procesos reales de Electron con perfiles aislados y dispositivos de medios sintéticos. Recorre crear/conectar, verifica que no haya solicitud de micrófono antes de Unirse, recibe RTP de audio en ambos extremos y comprueba energía de audio decodificado, silencio, salida/reentrada, persistencia cifrada, reconexión y pérdida del servidor. El audio entrante se silencia en el elemento de reproducción durante la prueba para no emitir los tonos por los parlantes. Capturas y perfiles de prueba quedan en `work/`, ignorado por Git. No se registran invitaciones en las salidas de prueba.
- `package`: genera `release/win-unpacked/NIP.exe` y sus archivos acompañantes. Copiá **toda la carpeta** para usarla sin instalar. `dist:win` genera el instalador NSIS por usuario; `test:installer` valida instalación y desinstalación reales con perfil aislado. Ver [publicación y verificación](docs/RELEASING.md).
- `test:package`: después de empaquetar, abre el ejecutable de distribución real, crea y detiene un servidor sin activar micrófono y comprueba que la interfaz funcione dentro del ASAR.

**La prueba sintética no demuestra hablar/escuchar con micrófonos y parlantes físicos, ni conectividad entre dos máquinas o desde internet.** Esas verificaciones requieren prueba manual. Usá auriculares en la prueba real para evitar realimentación, comprobá ambos sentidos, silencio y cambio de dispositivos.

## Estructura y decisiones

Electron + TypeScript evita una segunda implementación de captura de audio y WebRTC en Windows. Node aloja la señalización dentro del proceso principal; Chromium maneja los medios. Es una base simple y mantenible para este MVP, con un costo de memoria/distribución mayor que una shell nativa. Dependencias fijadas y lockfile; `npm ci` para reproducibilidad.

```text
src/main.ts         Ciclo de vida, IPC, permisos y persistencia segura
src/preload.ts      API limitada expuesta al renderizador
src/server.ts       Servidor WSS, autenticación y sala
src/client.ts       Cliente WSS y pin TLS previo a autenticación
src/invitation.ts   Formato y validación de invitaciones
src/voice.ts        Captura de audio y pares WebRTC
src/renderer.ts     Flujos de la interfaz
tests/             Pruebas de integración y escritorio
```

Audio en malla: cada cliente envía audio a cada participante. Es razonable para un grupo pequeño; el tráfico por cliente crece con el grupo y el total cuadráticamente. No es una arquitectura para salas grandes. Si ese fuera el objetivo, un SFU autohospedado sería la evolución habitual, con mayor complejidad de operación. La única sala admite hasta 24 conexiones de señalización como límite defensivo, **no como capacidad de audio certificada**. Este MVP se valida con dos clientes.

Sin video, chat, múltiples canales ni migración automática de anfitrión. Pendientes antes de distribuir ampliamente: prueba de hardware/LAN real, conectividad WAN según la red del grupo, revisión de seguridad y firma de ejecutables.

Referencias: [seguridad de Electron](https://www.electronjs.org/docs/latest/tutorial/security), [safeStorage](https://www.electronjs.org/docs/latest/api/safe-storage), [RTCPeerConnection e ICE](https://developer.mozilla.org/en-US/docs/Web/API/RTCPeerConnection/RTCPeerConnection).

## Desarrollo guiado por especificaciones

Las skills originales de [Matt Pocock](https://github.com/mattpocock/skills) están incluidas en `.agents/skills/`, en alcance de proyecto. El flujo adoptado es acordar el problema y las decisiones con `grill-with-docs`, escribir la especificación con `to-spec`, dividirla en entregas verificables con `to-tickets` e implementar con `implement`, apoyada en `tdd` y `code-review`. No hay una skill llamada SDD ni se generaron especificaciones de funcionalidades futuras.

Ver [CONTRIBUTING.md](CONTRIBUTING.md) para las convenciones del proyecto y [procedencia de las skills](docs/agents/skills.md) para la lista exacta, versión y licencia.

## Licencia

NIP es open source bajo [MIT](LICENSE), copyright Santiago Abregu. Las skills de terceros conservan la [licencia MIT de Matt Pocock](.agents/skills/LICENSE.mattpocock). Las dependencias y Electron conservan sus propias licencias; al distribuir ejecutables deben conservarse sus avisos correspondientes.
