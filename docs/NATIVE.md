# NIP nativo

Implementación Windows x64 en Rust: egui/eframe y AccessKit para la interfaz, CPAL/WASAPI para audio, Opus mono a 48 kHz y QUIC/TLS 1.3 con quinn/rustls. No incorpora Chromium ni WebView.

La interfaz usa el driver OpenGL de Windows. El smoke gráfico del runner alojado utiliza [Mesa llvmpipe](https://github.com/pal1000/mesa-dist-win/blob/master/readme.md) con versión y checksum fijados, inyectado sólo en el directorio temporal de prueba después de instalar. Esas DLL no forman parte del instalador ni de las mediciones locales.

La distribución nativa 0.2.0 usa un protocolo nuevo y se instala junto a Electron v0.1.2. La implementación nativa y la versión anterior se conservan en el repositorio; no hay migración automática de perfiles.

## Desarrollo sin reinstalar

Requiere Rust mediante rustup, Visual Studio Build Tools con C++/Windows SDK y CMake. `native/rust-toolchain.toml` fija Rust 1.100.0; Cargo.lock fija las dependencias. NSIS sólo se necesita para generar el instalador.

Desde la raíz del repositorio, en dos terminales PowerShell:

```powershell
.\scripts\dev-native.ps1 -Profile anfitrion -Release
.\scripts\dev-native.ps1 -Profile amigo -Release
```

Cada comando compila incrementalmente y abre un perfil independiente en `work/native-<nombre>`. El desarrollo usa `--local-only` por defecto: listeners y destinos se restringen a loopback para no solicitar acceso de firewall a cada binario QA. Para una prueba LAN/WAN explícita, agregá `-AllowNetwork`; la app instalada conserva su funcionamiento de red habitual. NIP no modifica reglas de Windows. Cerrá y volvé a ejecutar después de editar. No requiere instalación ni utiliza tus datos de Electron. Para compilar más rápido durante desarrollo, omití `-Release`.

## Uso y conectividad

Elegí tu nombre y creá un servidor, o conectate con una invitación. El micrófono permanece apagado hasta entrar a la llamada o iniciar explícitamente la prueba privada. El chat funciona fuera de llamada: texto plano de hasta 2000 caracteres Unicode, cinco mensajes cada diez segundos por participante e historial de los últimos cien mensajes en memoria del anfitrión. En servidores de sesión el historial desaparece al detenerlos. Los persistentes conservan los mensajes localmente y permiten cargar los anteriores; ver [Servidores persistentes](PERSISTENT-SERVERS.md).

Las invitaciones `nip-native://join#...` se pegan dentro de la app. Contienen un certificado de confianza y un permiso aleatorio de 256 bits; son credenciales compartidas, no identificaciones personales. Cualquiera que reciba el enlace puede reenviarlo. No hay revocación individual ni expiración automática.

La conexión directa necesita un puerto UDP del anfitrión alcanzable (45454 por defecto). Para LAN se usa su IP numérica; para otra casa puede requerir redirección de puertos y no funcionar con CGNAT. Alternativamente, cliente y anfitrión pueden abrir conexiones TLS/TCP salientes hacia un relay autoalojado con certificado fijado. No hay servicio público preconfigurado ni WAN validada; NIP no modifica router o firewall. Ver [Relay](RELAY.md).

La navegación separa servidores, canales de texto y salas de voz con IDs estables. El administrador del perfil local crea, renombra y elimina canales; conocer la invitación o llamarse administrador no concede ese permiso. Un clic en voz entra o cambia de sala; respeta entrar silenciado y conserva mute/deafen/PTT durante el cambio. Seleccionar texto no cambia la llamada. Cada canal conserva su borrador; Enter envía y Shift+Enter inserta una línea. Se conserva una conexión de cliente activa y varios anfitriones propios. Cambiar de servidor descarta los borradores para evitar otro destino.

Las invitaciones proponen una dirección de red local cuando existe, nunca loopback automáticamente. El diálogo identifica las direcciones que sólo funcionan en esta PC o en la misma red/VPN. Para otra casa hay que ingresar la IP pública y el puerto UDP accesible, configurar el router y permitir la aplicación en el firewall. Generar una invitación **no valida** accesibilidad desde Internet ni resuelve CGNAT.

El anfitrión retransmite audio Opus a los participantes autenticados. TLS protege cada conexión; **el anfitrión puede escuchar el audio**. No se promete E2EE frente al anfitrión. El protocolo y las invitaciones son incompatibles con Electron v0.1.2: todos los participantes deben usar el cliente nativo.

## Audio y privacidad

**Ajustes** es una sección de la aplicación con navegación entre **Perfil**, **Permisos**, **Voz y audio**, **Llamadas**, **Servidores**, **Aplicación** y **Conexión**. El engranaje junto a NIP abre Perfil; el de la barra de llamada abre directamente Voz y audio. Perfil guarda el nombre por defecto para los próximos inicios/conexiones sin renombrar una sesión ya conectada. Escape o Cerrar ajustes vuelve a los canales. Salir de Voz y audio detiene una prueba privada activa.

Voz y audio incluye dos paneles desplegables de **Opciones avanzadas**. Micrófono ofrece activación por nivel (umbral −72 a −12 dBFS y pausa de 40 a 1000 ms) y filtro de graves a 80 Hz. Audio permite ajustar el volumen del monitor privado, proteger picos al mezclar voces y elegir bitrate Opus automático/16/32/64 kb/s. Se guardan por perfil y se aplican también durante la prueba privada. La compuerta no es un detector de voz ni supresión de ruido; la calidad Opus se aplica al envío, no al monitor local. Los perfiles previos conservan su comportamiento al migrar.

Los paneles avanzados agregan EQ de entrada y salida de 10 bandas (31 Hz–16 kHz, ±12 dB, Q 1,4, preamplificador −24 a +12 dB), presets y bypass. AEC3 recibe como referencia el último bloque colocado en la cola de reproducción de NIP; el retardo de entrada/salida es ajustable (0–500 ms). No cancela audio de otras apps. AGC2 permite ajustar margen (0–20 dB) y amplificación máxima (0–50 dB), sin tocar el volumen físico de Windows. La supresión tiene cuatro niveles; mayor supresión puede introducir artefactos. El procesamiento interno puede limitarse a 32 kHz o usar 48 kHz; el transporte continúa mono a 48 kHz y 20 ms.

Los perfiles Bajo consumo / Voz equilibrada / Mayor calidad cambian DSP, bitrate y complejidad Opus reales; cada parámetro sigue editable. VBR, DTX, FEC y pérdida prevista se aplican al codificador. El receptor incorpora secuencia, jitter acotado, intento FEC con el paquete siguiente y PLC limitado; no garantiza recuperar todo audio perdido. El diagnóstico mide microsegundos de DSP de entrada, no CPU total ni latencia acústica. JSON de preferencias de audio no incluye invitaciones ni identidad. El backend valida y revierte la configuración si falla su guardado.

Pulsar para hablar admite F1–F12 y Mouse 4/5, con lectura global sólo de la tecla elegida; al soltarla se cierra el envío. Silenciar, ensordecer y la prueba privada siempre prevalecen. Entrar silenciado es opcional. El volumen por persona (0–200%) y mute local afectan sólo la mezcla local y duran esa conexión; se limpian al desconectar. Probar salida reproduce un tono de dos segundos sin abrir el micrófono ni publicar audio.

Llamadas permite reconectar hasta cinco veces con espera creciente, sin entrar automáticamente a voz. Cancelar o desconectar detiene los intentos. La selección automática comprueba cambios cada dos segundos. La salida puede recuperarse con el dispositivo predeterminado; una entrada fallida o una topología modificada requiere reintento explícito para no abrir otro micrófono inesperadamente. No sobrescribe la selección guardada. La pantalla muestra qué dispositivos se abrieron realmente. Los sonidos de entrada/salida son opcionales y usan la salida local.

Aplicación ofrece inicio con Windows mediante la clave Run del usuario (esta copia y este perfil), minimizar al área de notificación al cerrar, avisos de mensajes en la barra de tareas y atajos globales separados para mute/deafen. No usa notificaciones con contenido de mensajes. El menú de bandeja permite abrir o salir; al salir realmente se detienen red y audio. Las preferencias nuevas tienen defaults de migración y permanecen cifradas en el perfil DPAPI.

Servidores configura puerto por defecto y máximo de 1–32 participantes, incluyendo anfitrión. Cada servidor propio guarda su puerto, modalidad e inicio automático; el puerto se modifica con ese servidor apagado y reducir el máximo sólo limita entradas nuevas. Regenerar invitaciones revoca los tokens anteriores sin cambiar el certificado; participantes autenticados pueden seguir conectados. El cierre espera a que QUIC libere las conexiones antes de informar servidor apagado, para permitir encenderlo inmediatamente.

Conexión muestra transporte y endpoint activos, destino pendiente, último fallo de conexión y métricas QUIC. Cancelar un intento conserva la sesión anterior. El trayecto puede ser directo o relay TLS/TCP; la dirección loopback del puente no se presenta como endpoint público. Estas métricas y el enlace UDP local no prueban firewall, CGNAT ni WAN.

La lista de servidores reserva espacio para un menú de tres puntos y trunca nombres largos con elipsis. Permite cambiar nombre, apagar el anfitrión local y borrar con confirmación. En servidores ajenos, cambiar nombre crea un alias local que sobrevive a reconexiones; no renombra el servidor remoto. Borrar un servidor propio elimina su identidad e historial local y apaga ese anfitrión si estaba activo: las invitaciones anteriores dejan de funcionar. Borrar uno ajeno sólo elimina su invitación local y desconecta esa sesión. Apagar el anfitrión no corta una conexión a otro servidor.

Permisos distingue acceso comprobado durante una captura de permisos administrados por Windows. Ofrece accesos a la configuración del micrófono y Seguridad de Windows, sin afirmar que una conexión local pruebe reglas de firewall ni alcance externo. Usa las [URI oficiales de Configuración de Windows](https://learn.microsoft.com/en-us/windows/apps/develop/launch/launch-settings); NIP no cambia concesiones automáticamente.

Entrada y salida seleccionan los dispositivos de NIP. Nivel del micrófono y volumen de la app ajustan señales dentro de NIP, de 0 a 100%; no cambian los niveles globales de Windows. Las preferencias se guardan localmente. La ayuda de cada fila se abre con hover, foco de teclado o clic; Escape o clic afuera la cierra.

La prueba privada reproduce localmente la señal procesada y suspende el envío antes de habilitar el monitor. Al terminar restaura el mute anterior. Ensordecer detiene la prueba, la captura y la reproducción remota; desactivarlo conserva el mute previo. No se graban archivos de audio. La persistencia de chat se configura por servidor y no guarda llamadas. Usá auriculares para evitar realimentación.

El backend prefiere dispositivos a 48 kHz y remuestrea formatos de 8–192 kHz con filtro sinc/antialias, preservando mono a 48 kHz en el transporte. Puede escuchar sin una entrada utilizable. Tras un fallo o cambio automático de topología mantiene modo escucha y exige una acción explícita para reintentar captura. Sonora 0.3.0 implementa AEC3, supresión y AGC2 opcionales. Las pruebas sintéticas no certifican micrófonos, Bluetooth, sonido audible ni calidad acústica.

Los nombres de dispositivos activos indican frecuencias físicas y conversión. Ajustes distingue errores de enumeración/apertura, dispositivo ausente y falta de frames. El medidor logarítmico muestra voz baja; enumerar dispositivos o recibir frames no demuestra que el sonido sea audible.


Las discontinuidades recuperables de WASAPI (`Xrun`) se cuentan en el diagnóstico y no cierran el micrófono. `DeviceChanged` y `RealtimeDenied` tampoco son fallos fatales del stream. Los fallos reales conservan el detalle de CPAL y distinguen entrada de salida. Los callbacks de streams retirados no pueden detener un dispositivo de reemplazo. Si el dispositivo abre pero no entrega frames completos durante tres segundos, la captura se detiene con un mensaje de recuperación; no se deja una prueba aparentemente activa para siempre.

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

La verificación interactiva recorrió las ocho combinaciones de escritorio/Inicio/apertura. Se comprobaron existencia y destino de los accesos, presencia de la ventana cuando se solicita abrir y ausencia del proceso cuando se desmarca. Las instalaciones de prueba usan un registro y nombres independientes y se desinstalan al terminar.

También se verificó el portapapeles en la UI real de Windows con dos perfiles aislados: pegar texto Unicode, generar una invitación en el anfitrión, pulsar **Copiar invitación**, pegarla con Ctrl+V en el segundo cliente y conectar. La sala mostró a ambos participantes fuera de llamada y sin micrófono. La credencial no se imprimió ni se incluyó en capturas. Se cerraron ambos clientes y el servidor temporal al terminar. Esta comprobación usó un build local del mismo código nativo del tag `native-v0.2.0`; no requirió cambiar el binario ni publicar otra versión.

## Medición local

Muestra de 10 segundos por aplicación con perfil nuevo, ventana de inicio, sin servidor ni audio; tres segundos de estabilización. Equipo con seis procesadores lógicos. No es un benchmark de llamadas ni un promedio de múltiples ejecuciones.

| Métrica | Electron v0.1.2 | Rust nativo |
|---|---:|---:|
| Procesos | 6 | 1 |
| Working set total | 493,13 MiB | 86,52 MiB |
| Bytes privados totales | 228,88 MiB | 83,96 MiB |
| CPU sobre un núcleo | 0,000% | 0,766% |
| CPU sobre el equipo | 0,000% | 0,128% |
| Primer handle de ventana | 509 ms | 145 ms |

El working set sumado puede contar páginas compartidas varias veces. Los bytes privados se informan por separado. El arranque mide aparición del handle de ventana, no disponibilidad para interacción ni latencia de audio. Script reproducible: `scripts/benchmark-native.ps1`; resultado local completo: `work/native-benchmark.json`.

La tabla corresponde a la última ejecución con el build final. Una muestra anterior dio 586,41 MiB de working set para Electron y 141,67 MiB para el nativo; las condiciones del escritorio y las cachés introducen variación. No se presenta una única muestra como garantía de consumo.

Durante la última muestra había un aviso de Windows Security en el escritorio. Las ventanas se midieron sin interacción; la oclusión puede cambiar su actividad. Las muestras no demuestran una reducción consistente de CPU. El ahorro observado aquí corresponde a memoria y tamaño de distribución; no se extrapola a llamadas ni a otros equipos.

## Candidato local 0.3.0-beta.1: canales, adjuntos y respaldo

El historial anterior migra a `general` sin cambiar identidad ni invitaciones. El protocolo versionado rechaza clientes incompatibles con un error explícito. Voz usa pertenencia autoritativa y generaciones de transporte para descartar audio de otra sala.

Los adjuntos se revisan antes del envío y se almacenan en el anfitrión sólo en servidores persistentes; los de sesión ofrecen conversión explícita. Los límites iniciales son 100 MB por archivo (100 000 000 bytes) y 10 GB por servidor (10 000 000 000 bytes), configurables. No se borra contenido confirmado automáticamente. Las cuotas incluyen reservas concurrentes; bajarlas bajo el uso impide nuevas cargas. Los archivos originales se guardan como blobs locales con IDs internos y SHA-256; no se abren ni ejecutan automáticamente. Los blobs locales no están cifrados con DPAPI.

Las cargas y descargas usan streams QUIC fiables separados, buffers acotados y prioridad inferior a voz. La publicación ocurre después de confirmar almacenamiento e historial; un outbox permite recuperar el reinicio entre ambos. Un reintento con el mismo ID no duplica el mensaje. Cancelar deja sin confirmar una carga pendiente; si ya ganó la confirmación, el contenido permanece. La UI informa esa carrera.

Borrar un canal exige confirmación, elimina sólo su historial y archivos y preserva al menos un texto. No elimina descargas o respaldos previos. Los archivos alojados se descargan únicamente mientras el anfitrión está online.

Perfil incluye exportación/importación con contraseña y destino nuevo offline; ver [Respaldo portable](NATIVE-BACKUP.md). El paquete completo se valida según [Distribución](NATIVE-DISTRIBUTION.md). Las pruebas locales no certifican WAN, hardware físico, restauración en otro equipo ni Windows limpio.
