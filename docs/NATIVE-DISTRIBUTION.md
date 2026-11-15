# Candidato nativo y actualización manual

El candidato local se construye con el working tree actual, incluidos cambios sin commit. No es una publicación ni una beta validada. El paquete incluye canales, adjuntos y restauración; las pruebas físicas y de entornos externos conservan sus propios gates.

## Construcción e identificación

En Windows con Rust/MSVC/CMake y NSIS disponibles:

```powershell
.\scripts\package-native.ps1 -TestBuild
.\scripts\test-native-installer.ps1 -TestBuild
```

La primera orden produce el instalador de prueba y el ejecutable independiente del servicio relay en `work/native-installer-test`. El relay no se instala ni se inicia automáticamente con la app; su operador debe configurarlo según `docs/RELAY.md`. Sin `-TestBuild`, la salida es `release/native`; construir allí no publica archivos ni instala la aplicación. La versión se obtiene de `native/Cargo.toml`. Electron conserva su propia versión en `package.json`.

`candidate-manifest.json` registra versión, target, Cargo/Rust, commit, árbol con cambios, huella SHA-256 de entradas nativas, Cargo.lock y hashes/tamaños del binario, instalador y licencias. La huella incluye archivos nativos conocidos por Git y archivos nuevos no ignorados, además del script de empaquetado y LICENSE. No reemplaza conservar el working tree exacto: un commit con cambios locales pendientes no basta para reconstruirlo. El script rechaza cambios en esas entradas durante el empaquetado.

`-SkipBuild` conserva `buildSkipped: true` en el manifest. Sirve para empaquetar un binario existente; no demuestra que ese binario corresponda al código actual. Para un candidato integrado usar el build completo con árbol estable.

`SHA256SUMS.txt` identifica instalador, servicio relay y licencias. El paquete no contiene claves ni configuración del relay. Se puede verificar el instalador antes de ejecutarlo con:

```powershell
Get-FileHash -Algorithm SHA256 -LiteralPath .\work\native-installer-test\NIP-Native-Setup-0.3.0-beta.1-x64.exe
Get-AuthenticodeSignature -LiteralPath .\work\native-installer-test\NIP-Native-Setup-0.3.0-beta.1-x64.exe
```

Usar el nombre de la versión registrada en el manifest si cambia. Un hash sólo detecta diferencias frente a un valor obtenido por un canal de confianza; no acredita editor ni firma. La firma requiere certificado e infraestructura disponibles y no se presupone. No hay autoactualizador comprometido.

## Gates de instalación y actualización

El smoke local usa nombre, registro, ejecutable y perfil aislados de prueba. Comprueba instalación, ventana, salida acotada, desinstalación, conservación de un marcador de perfil y que el ejecutable Electron existente no cambie. Esto no equivale a Windows limpio ni prueba una actualización con servidores e historial.

Para cada candidato final registrar el manifest y el hash exactos junto a:

1. Instalación en Windows limpio sin Rust, Node, Visual Studio ni DLL de desarrollo; apertura y cierre desde bandeja.
2. En perfil QA, crear servidor persistente, mensajes, canales y adjuntos; guardar inventario e invitación sin publicarla. Cerrar NIP y respaldar antes de actualizar.
3. Instalar el candidato sobre la versión anterior con NIP cerrado. Abrir el mismo perfil QA y comprobar identidad, invitación, preferencias, canales e historial, y descargar adjuntos idénticos.
4. Probar inicio automático en ese entorno QA y desactivarlo al terminar. Desinstalar y comprobar conservación de datos; reinstalar y volver a abrir el perfil.
5. Restaurar un backup del candidato en perfil aislado, inicialmente offline y sin autoarranque. Comprobar inventario antes de activar la copia.

El instalador nativo conserva `%APPDATA%\NIPNative` al desinstalar. La copia DPAPI actual depende de la cuenta Windows; hasta validar la exportación cifrada portable no atribuirle portabilidad. No usar el perfil real del usuario para estos gates.

En cada versión repetir construcción, instalación/actualización y backup/restauración con archivos reales. WAN entre redes independientes, hardware/escucha humana, lector de pantalla y firma mantienen su propia evidencia; un paquete generado no los certifica. Publicar instaladores o releases requiere autorización explícita.

El candidato actual es 0.3.0-beta.1 (protocolo 2). El instalador de prueba pasa `--local-only` y un perfil QA propio tanto desde su pantalla final como desde sus accesos directos. No abre el perfil nativo real por defecto. Los smokes lanzan la app con scope loopback sin solicitar reglas de firewall.
