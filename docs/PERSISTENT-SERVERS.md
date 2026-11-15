# Servidores locales de sesión y persistentes

Cada servidor propio tiene una identidad y una configuración independientes. Se pueden alojar varios a la vez, usando puertos UDP distintos. El administrador puede visitar otro servidor o salir de la sala sin apagar los que aloja.

## Modalidades

- **De sesión:** conserva identidad, invitaciones, nombre y configuración para volver a encenderlo. Su chat es temporal y sólo mantiene los últimos 100 mensajes en memoria.
- **Persistente:** además guarda todos los mensajes nuevos en SQLite local antes de retransmitirlos como enviados. Al cerrar o apagar conserva el historial. Al abrir carga los últimos 100; los clientes pueden solicitar mensajes anteriores en tandas de 100.

Al crear uno persistente se activa **Iniciar este servidor al abrir NIP**. Se puede desactivar en Ajustes → Servidores. Abrir NIP puede iniciar varios servidores persistentes sin conectar al administrador a ninguna sala y sin activar su micrófono. NIP debe seguir ejecutándose, visible o en la bandeja; con el equipo apagado/suspendido o la app cerrada, el servidor está fuera de línea.

“Persistente” no significa que exista un servidor en la nube ni que se haya resuelto la accesibilidad desde otra casa. Siguen aplicando las restricciones de UDP/router/CGNAT documentadas en NATIVE.md.

## Convertir un servidor existente

Desde los tres puntos → Configurar servidor, o Ajustes → Servidores, seleccionar el servidor, cambiar su modalidad y pulsar **Guardar configuración del servidor**.

Convertir uno encendido a persistente conserva los mensajes que todavía están disponibles en memoria (hasta 100) y guarda todos los nuevos desde ese momento. Los mensajes temporales que ya se perdieron no se pueden recuperar. Convertirlo a sesión detiene el guardado de mensajes nuevos pero conserva el historial anterior en disco; volver a persistente permite consultarlo nuevamente.

El puerto se modifica con ese servidor apagado; los otros servidores pueden continuar activos. Reducir participantes sólo limita nuevas conexiones. Apagar no borra datos. Borrar, con confirmación, detiene únicamente ese servidor y elimina identidad, invitaciones e historial local. No puede retirar copias de mensajes que otros participantes ya hayan conservado.

## Datos, respaldo y compatibilidad

El catálogo de identidades, tokens y configuración está en `native-secrets.dpapi` dentro del perfil. Cada historial reside en `servers/<ID del certificado>/history.sqlite3`, con WAL y sincronización FULL. El contenido de cada mensaje usa DPAPI del usuario de Windows; los índices de mensajes y remitentes son metadatos de SQLite. No es cifrado de extremo a extremo: el administrador controla y puede leer el historial desde NIP.

Preferí la exportación cifrada desde Perfil y verificá restaurándola en una carpeta nueva offline; ver [Respaldo portable](NATIVE-BACKUP.md). Para una copia manual bajo la misma cuenta, cerrar NIP y copiar la carpeta completa del perfil, incluidos SQLite/WAL y adjuntos. DPAPI requiere la cuenta Windows correspondiente; copiar la carpeta a otra cuenta/equipo no constituye una exportación portable. No abrir el perfil migrado con una versión anterior que no conozca el catálogo múltiple.

Los perfiles anteriores con un único anfitrión se migran conservando certificado, token, nombre, invitación y puerto. Antes de migrar se crea `native-secrets.legacy.dpapi` si todavía no existe. El servidor anterior permanece de sesión y no se inicia automáticamente: el usuario elige si convertirlo.

No se persisten llamadas activas, micrófonos abiertos, conexiones de participantes ni audio. Los canales tipados e IDs estables se conservan en SQLite; el historial y los adjuntos confirmados se retienen en modalidad persistente. Borrar un canal elimina únicamente su contenido y exige mantener un canal de texto.

## Validación

Las pruebas cubren historial SQLite paginado, escritura fallida sin insertar mensajes, migración de identidad anterior, conversión con mensajes vivos, cierre y reapertura del perfil, inicio automático sin micrófono, entrada con la invitación anterior, dos servidores propios independientes, chat de sesión vacío tras reiniciar y borrado del historial propio sin afectar al otro servidor.
