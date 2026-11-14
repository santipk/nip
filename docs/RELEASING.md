# Publicar el cliente nativo

Requiere Windows x64, Rust/rustup, MSVC/Windows SDK, CMake y NSIS. Ejecutar desde `native/`: `cargo fmt --all -- --check` y `cargo test --release --locked`. Desde la raíz: `scripts/package-native.ps1 -TestBuild` y `scripts/test-native-installer.ps1 -TestBuild`. El build de prueba usa un producto y registro separados: nunca reemplazar una instalación real para verificar un instalador.

Recorrer las ocho combinaciones de escritorio/Inicio/apertura en la pantalla final. Comprobar destino de ambos accesos y ejecución del binario instalado cuando se selecciona abrir. Desinstalar conserva el perfil y debe eliminar sólo los archivos y accesos del producto de prueba. La validación física de audio y WAN requiere equipos/redes reales; distinguirla de las pruebas sintéticas.

El workflow `Native Windows` ejecuta las pruebas, empaqueta el binario, instala/abre/desinstala en un runner limpio y conserva EXE, checksum y avisos de dependencias. No publica automáticamente. Para publicar una versión, actualizar Cargo.toml/Cargo.lock, agregar notas en `docs/releases/native-v<version>.md`, revisar el resultado del workflow y crear el tag `native-v<version>`. Descargar el artefacto de la ejecución verificada y publicar sus archivos sin reconstruirlos localmente. La primera distribución nativa se marca prerelease. Verificar el checksum del archivo público después de subirlo.

No hay autoactualizador. El instalador nativo usa su propio producto, registro, accesos y perfil; la versión Electron puede permanecer instalada. Firma pendiente: [Code signing policy](../CODE_SIGNING.md). No agregar una integración ficticia ni publicar como firmado un archivo sin Authenticode válido.

## Proceso histórico de Electron v0.1.x
# Releases Windows

Requiere Windows x64, Node 22.12+ y npm. Ejecutar `npm ci`, `npm run typecheck`, `npm test`, `npm run dist:win` y `npm run test:installer`. Electron Builder genera `release/NIP-Setup-<version>-x64.exe` y la carpeta `release/win-unpacked/`. La configuración usa una lista explícita de archivos, ASAR y dependencias de producción; no incorpora perfiles, tests ni skills.

El instalador NSIS instala por usuario, sin elevación, crea accesos directos y registra la desinstalación en Windows. No inicia NIP automáticamente, no modifica firewall/router ni instala servicios o tareas de inicio. Se desinstala desde Configuración > Aplicaciones > NIP. La desinstalación conserva los datos de Electron; borrarlos es una acción manual separada. La actualización es manual: cerrar NIP y ejecutar el instalador de la nueva versión.

`test:installer` usa un directorio de instalación y un perfil aislados dentro de `work/`. Se niega a ejecutar si existe una instalación o acceso directo NIP para evitar reemplazar datos del usuario. Comprueba instalación, accesos directos, registro HKCU, arranque y servidor real, desinstalación y conservación del perfil aislado. La retención del perfil normal se configura con `deleteAppDataOnUninstall: false`; no se borra ni utiliza el perfil real durante la prueba. No verifica SmartScreen, audio físico ni redes externas.

Para publicar: actualizar versión y lockfile, agregar `docs/releases/v<version>.md`, revisar cambios, hacer commit, crear el tag `v<version>` y subirlo. El workflow `Windows installer` compila y prueba en GitHub-hosted Windows, conserva el artefacto y publica EXE y `SHA256SUMS.txt` en Releases sólo si pasan las verificaciones. `workflow_dispatch` construye y prueba sin publicar. El tag debe coincidir con la versión. No hay autoactualizador, así que no se publican metadatos de actualización.

Estado de firma y siguiente paso SignPath: [Code signing policy](../CODE_SIGNING.md). No hay token ni paso de firma ficticio. Después de la aceptación, integrar la acción oficial usando el ID del artefacto y restringir la firma a tags/revisión/aprobación manual. No firmar artefactos locales como si provinieran de CI.
