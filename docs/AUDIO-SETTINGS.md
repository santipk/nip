# Ajustes de audio y contribuciones

La interfaz de Ajustes es la vía de configuración del motor nativo. Las preferencias de audio se pueden exportar/copiar/importar como JSON desde **Voz y audio → Opciones avanzadas → Perfiles de audio**. El formato es `AudioSettings` en `native/src/contracts.rs`; no incluye credenciales del servidor. El backend valida antes de persistir. Los campos nuevos tienen defaults para perfiles anteriores.

## Cadena de señal

Entrada a 48 kHz mono → Sonora (AEC3 / supresión / AGC2 opcionales) → EQ del micrófono → filtro de 80 Hz y nivel manual → compuerta por nivel → Opus → envío autorizado. Pulsar para hablar, silenciar, ensordecer y la prueba privada controlan la publicación independientemente del DSP. Una prueba privada nunca publica audio.

Recepción Opus → volumen/mute por participante → suma de voces → EQ de salida → volumen de la app y protección de picos → salida. El monitor privado recibe la entrada procesada y agrega su volumen independiente. La referencia AEC es el bloque anterior colocado en reproducción por NIP; el retardo estimado es configurable. Sonidos de prueba y avisos usan un stream de salida independiente, sin abrir entrada ni enviarse al grupo.

## Parámetros expuestos

| Grupo | Parámetro | Rango / comportamiento |
| --- | --- | --- |
| Dispositivos | Entrada y salida | Nombre guardado o predeterminado de Windows; se prefiere 48 kHz, con remuestreo de formatos entre 8 y 192 kHz |
| Niveles | Entrada / app / monitor | 0–100%; afectan solamente a NIP |
| Transmisión | Pulsar para hablar | F1–F12 o Mouse 4/5; cierra envío al soltar; mute/deafen prevalecen |
| Transmisión | Activación por nivel | Umbral −72 a −12 dBFS; pausa 40–1000 ms; no clasifica habla |
| Micrófono | Filtro de graves | Pasa altos de primer orden a 80 Hz |
| WebRTC | Supresión | Desactivada o baja/moderada/alta/muy alta; mayor supresión puede deformar voz |
| WebRTC | AEC3 | Referencia de reproducción de NIP; retardo 0–500 ms; no incluye audio de otras apps |
| WebRTC | AGC2 digital | Margen 0–20 dB; ganancia máxima 0–50 dB; no cambia volumen físico de Windows |
| WebRTC | Frecuencia interna | Hasta 32 kHz o 48 kHz; transporte siempre 48 kHz |
| EQ de entrada y salida | 10 bandas | 31/63/125/250/500 Hz y 1/2/4/8/16 kHz; Q 1,4; ±12 dB |
| EQ de entrada y salida | Preamplificación | −24 a +12 dB; bypass y presets Plano / Claridad / Cálida |
| Mezcla | Protección de picos | Reduce la ganancia del bloque si supera el máximo; no es un limitador con lookahead |
| Opus | Bitrate | Automático / 16 / 32 / 64 kb/s |
| Opus | Complejidad | 0–10; más cálculo para codificar a valores mayores |
| Opus | VBR / DTX | Bitrate variable y transmisión discontinua durante silencio |
| Opus | FEC / pérdida prevista | Redundancia de envío y pérdida prevista 0–100%; el receptor intenta FEC con el paquete siguiente y usa PLC cuando falta audio |
| Participantes | Volumen / mute local | 0–200%; por conexión; no altera otros clientes |

La prueba de micrófono muestra nivel original antes de filtros y nivel procesado antes de la compuerta, junto al nivel original en dBFS y un aviso de saturación de entrada. Esto distingue una entrada física baja de una atenuación por procesamiento.

Los perfiles de consumo cambian los parámetros visibles; no hay un procesamiento oculto adicional. El diagnóstico informa tiempo de procesamiento de entrada por bloque de 20 ms, no CPU total ni latencia extremo a extremo. Al subir bandas, reducir la preamplificación ayuda a evitar recorte.

## Límites actuales

No hay audio estéreo de transporte, selección de backend ASIO ni relay público integrado. No se ofrecen controles operativos para esas capacidades ausentes. La conectividad WAN necesita una prueba desde otra red; escuchar UDP localmente no la demuestra.

Sonora 0.3.0 es una dependencia fijada y opcional en tiempo de ejecución (DSP apagado por defecto). Sus pruebas de señal no certifican por sí solas la calidad acústica en hardware real. Los textos de licencias, incluidos Sonora y sus subcrates BSD, se incluyen por `scripts/package-native.ps1` desde el grafo Cargo.lock.

## Al añadir un parámetro

1. Agregar campo/default y validación en los contratos y `audio.rs`; mantener perfiles anteriores legibles.
2. Implementar su efecto en `dsp.rs` o el motor, respetando los gates de privacidad y los callbacks sin bloqueos de audio.
3. Exponerlo en Ajustes con unidades, alcance, límites y restablecimiento. Mantener exportación/importación.
4. Verificar con señales y una prueba de integración que observe el efecto, no sólo el valor guardado.
5. Probar captura/salida física y registrar sus límites por separado de las pruebas sintéticas.

## Escucha cuando falla la entrada

La salida de llamada puede abrir aunque no exista un micrófono utilizable. Un fallo de entrada o tres segundos sin muestras detienen sólo esa captura: la llamada sigue en modo escucha, con la causa visible en el diagnóstico. Silenciar/desilenciar no reactiva esa entrada. Para reintentar, volver a entrar a voz, seleccionar explícitamente otro dispositivo o iniciar una prueba privada. Enumerar dispositivos y abrir ajustes no reabren una entrada fallida. La salida ausente sigue impidiendo escuchar y muestra su error.

El modo output-only sintético valida recepción Opus y gates de publicación; no valida dispositivos físicos ni sonido audible.

## Formato físico y remuestreo

La selección prefiere un formato físico a 48 kHz y, si no existe, el más cercano disponible entre 8 y 192 kHz. Entrada y salida pueden usar frecuencias diferentes; el DSP y Opus conservan mono a 48 kHz. Los nombres activos muestran ambas frecuencias y el sentido de conversión. No se cambia el formato global de Windows.

`fixed-resample` 0.13.0, sobre Rubato, realiza conversión sinc con antialias y buffers preasignados. Se conserva su retardo de filtro en lugar de recortar el inicio; al cambiar los gates se descarta el estado anterior del filtro para impedir que muestras de una prueba o captura previa reaparezcan. La salida sintética y las pruebas de tonos no certifican los dispositivos físicos a 44,1/48/96 kHz, USB o Bluetooth.

## Pérdida y reordenamiento

Cada paquete de audio lleva una secuencia interna de 64 bits. El receptor espera dos bloques de 20 ms al comenzar, ordena hasta ocho paquetes por participante y descarta duplicados y paquetes ya reproducidos. Ante una pérdida aislada intenta FEC usando el paquete siguiente sin consumirlo; Opus puede recurrir a PLC si ese paquete no contiene redundancia. Sin nuevos paquetes se limita la disimulación a tres bloques y después se deja silencio. No se garantiza recuperar todo audio perdido ni calidad audible bajo WAN.

La transición de sala tiene un gate de transporte adicional: invalida la generación del codificador y cierra envío hasta la confirmación del servidor, sin cambiar mute, deafen o PTT ni abrir otra captura. La prueba privada sigue sin publicar. Los contadores sintéticos informan intentos FEC, huecos disimulados y reordenamiento; un intento FEC no certifica que existiera redundancia ni que se recuperase audio original.
