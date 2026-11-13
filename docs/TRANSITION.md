# Transición a Rust nativo

El usuario decidió migrar completamente a Rust sin Chromium ni WebView. La release Electron pública v0.1.2 continúa disponible mientras se implementa y verifica la nueva aplicación. No instalar prototipos sobre la instalación real.

Checkpoint Electron no publicado: instalador asistido con página final y tres opciones independientes (compilado, validación UI incompleta); scrollbar CSS; servidor/cliente de chat con dos pruebas WSS nuevas que pasan, sin UI de chat terminada; preview web dev con adaptador de demostración, sin validación visual completa; motor experimental de ajustes/prueba privada/deafen, sin interfaz ni validación de privacidad completadas. Estos archivos se conservan para recuperación, no representan una release lista ni paridad funcional.

La nueva implementación debe cubrir chat, audio, mute/deafen/restauración, monitor privado sin fuga, configuración de dispositivos/niveles/volumen y ayudas accesibles, identidad cromática y menos cajas, desarrollo incremental sin reinstalar, instalación clara. Se documentarán protocolo y transición de datos. Los perfiles Electron no se borran ni sobrescriben; la importación no se presupone.
