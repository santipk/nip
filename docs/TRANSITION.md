# Transición a Rust nativo

El usuario decidió migrar completamente a Rust sin Chromium ni WebView. La release Electron pública v0.1.2 continúa disponible mientras se implementa y verifica la nueva aplicación. No instalar prototipos sobre la instalación real.

Checkpoint Electron no publicado: instalador asistido con página final y tres opciones independientes (compilado, validación UI incompleta); scrollbar CSS; servidor/cliente de chat con dos pruebas WSS nuevas que pasan, sin UI de chat terminada; preview web dev con adaptador de demostración, sin validación visual completa; motor experimental de ajustes/prueba privada/deafen, sin interfaz ni validación de privacidad completadas. Estos archivos se conservan para recuperación, no representan una release lista ni paridad funcional.

La implementación nativa cubre chat, audio, mute/deafen/restauración, monitor privado, dispositivos/niveles/volumen y ayudas accesibles. Incluye ejecución incremental con perfiles separados e instalador independiente. Las pruebas de protocolo, audio sintético entre dos clientes e instalación silenciosa aislada pasaron. La revisión final está corrigiendo casos de historial máximo, conexión pendiente y muestras residuales; falta terminar la matriz interactiva de ocho combinaciones del instalador. No se declara verificado hardware físico ni WAN. Ver [documentación nativa](NATIVE.md).

Los perfiles Electron no se borran ni sobrescriben. El cliente nativo usa un perfil nuevo y un protocolo incompatible; no se implementa importación automática.
