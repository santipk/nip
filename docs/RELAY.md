# Relay autoalojable: diseño y límites

Estado: transporte integrado con la app y probado localmente; no hay servicio público ni conectividad WAN validada.

## Transporte evaluado

| Alternativa | Ventaja | Coste para NIP |
| --- | --- | --- |
| TURN | Protocolo estándar de relay y asignación de permisos | Requiere integrar asignaciones, credenciales y un socket adaptado al transporte QUIC actual. El relay UDP no resuelve por sí solo una red que bloquea UDP al cliente. |
| WebSocket seguro | Puede usar infraestructura HTTP/TLS existente | Agrega upgrade/framing HTTP; hereda la entrega ordenada de TCP. |
| Paquetes QUIC opacos sobre TLS/TCP | Conserva QUIC y su certificado entre cliente y anfitrión, con conexiones salientes desde ambos extremos | TCP puede retrasar voz detrás de paquetes perdidos o transferencias; requiere medición y no promete latencia equivalente a QUIC directo. |

La tercera alternativa funciona en pruebas locales con dos clientes QUIC reales. Su aceptación para voz WAN queda condicionada a pruebas de caída, pérdida y carga. No se reemplaza el protocolo de la aplicación por texto plano ni se desactiva la verificación de certificados.

El protocolo TURN está documentado en [RFC 8656](https://www.rfc-editor.org/rfc/rfc8656). WebSocket se especifica sobre TCP en [RFC 6455](https://www.rfc-editor.org/rfc/rfc6455). Quinn ofrece [sockets abstractos](https://docs.rs/quinn/latest/quinn/trait.AsyncUdpSocket.html) y endpoints UDP; el puente propuesto usa sockets loopback y evita modificar su implementación TLS. La capa TLS exterior utiliza [tokio-rustls](https://docs.rs/tokio-rustls/0.26.6/tokio_rustls/), con certificado del relay fijado explícitamente.

## Confianza y amenazas

Hay dos autenticaciones separadas: el relay valida credenciales diferentes para anfitrión y miembros; dentro del túnel, QUIC verifica el certificado del anfitrión y NIP valida la invitación. La credencial para registrar un anfitrión nunca debe incluirse en una invitación de miembro. El relay no sustituye la autorización de canales o adjuntos.

El operador puede observar IP de origen, ruta solicitada, horarios, duración y volumen de tráfico. También puede interrumpir, retrasar o descartar tráfico. No dispone de la clave privada del anfitrión y el contenido de NIP permanece dentro de QUIC cifrado. El anfitrión sí accede al contenido, como en la conexión directa.

Se necesitan límites de sesiones, tamaño de paquete, tiempo de autenticación y tráfico; colas acotadas y limpieza al cancelar. La caída del relay debe mostrarse como fallo de transporte sin reactivar captura. Una prueba local no demuestra disponibilidad pública, redes independientes ni comportamiento bajo pérdida WAN.

## Servicio local y autoalojamiento

Desde `native`, con Cargo/MSVC disponibles:

```powershell
cargo run --locked --bin nip-relay -- init C:\NIP-Relay-QA 127.0.0.1:45455
cargo run --locked --bin nip-relay -- serve C:\NIP-Relay-QA\relay-server.json
```

`init` exige una carpeta nueva y genera una identidad TLS propia, dos secretos aleatorios distintos y tres JSON. `relay-server.json` contiene la clave privada del relay y ambas credenciales; sólo queda en el operador. `relay-host.json` contiene el secreto de registro del anfitrión; sólo se configura en el anfitrión. `relay-member.json` contiene el certificado fijado y el permiso de cliente compartible con miembros. No publicar los dos primeros archivos ni incluirlos en invitaciones. Protegé su carpeta con los permisos del sistema operativo; estos archivos de operación del servicio no usan DPAPI.

El ejemplo sólo escucha loopback. Para un equipo público propio, el operador debe elegir una interfaz TCP de escucha y un puerto accesible (por ejemplo `0.0.0.0:45455`); en los JSON de anfitrión y miembro debe sustituir la dirección por la IP numérica alcanzable del relay, preservando certificado/ruta y sus tokens respectivos. La app no contrata, publica ni abre firewall/router automáticamente. No se requiere UDP público en cliente/anfitrión: ambos conectan TCP/TLS de salida al relay. El servicio no aloja mensajes o archivos; el anfitrión sigue siendo necesario.

Los certificados fijados se verifican con los nombres internos `nip-relay.local` (relay) y `nip.local` (anfitrión), además de la cadena de confianza específica recibida. Cambiar la IP no deshabilita esa comprobación. Una rotación de certificado exige distribuir la configuración actualizada por un canal de confianza.

El servicio limita 128 rutas, 256 conexiones de autenticación/túnel, 1–32 clientes activos por ruta y una cola acotada de anfitriones por ruta. Autenticación y emparejamiento tienen 5 s; los paquetes encapsulados tienen un máximo de 2048 bytes y las lecturas inactivas vencen a los 60 s. Cada dirección aplica un presupuesto de 8 MiB/s por túnel, con backpressure TCP. El pool del anfitrión reintenta cada 500 ms al perder conexión. Estos límites son del transporte; no sustituyen cuotas de adjuntos ni prioridad de voz del protocolo NIP.

Detener el proceso (`Ctrl+C`) corta los túneles. Un cliente debe reconstruir el puente antes de reconectar QUIC; no se puede reutilizar una llamada activa ni reactivar captura automáticamente. Para operación permanente hacen falta supervisión de procesos, permisos de archivos, rotación de credenciales y diagnóstico del operador. No hay despliegue público realizado por estas instrucciones.
