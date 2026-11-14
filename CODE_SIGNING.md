# Code signing policy

## Estado actual

La distribución actual de NIP **no tiene firma Authenticode**. La solicitud a SignPath Foundation fue enviada y el formulario confirmó su recepción el 13 de noviembre de 2026 (hora de Argentina). La aprobación, el certificado y la integración de firma siguen pendientes. Windows/SmartScreen o el antivirus pueden mostrar advertencias. Una futura firma tampoco garantiza ausencia de advertencias.

## Responsables

Autor, mantenedor, revisor y futuro aprobador de releases: [Santiago Abregu (`santipk`)](https://github.com/santipk), propietario del repositorio. Los cambios de terceros requieren revisión del mantenedor. Cada solicitud de firma deberá tener aprobación manual y todos los participantes deberán usar MFA en GitHub y SignPath. La configuración de la cuenta SignPath se verificará al habilitar la integración.

## Alcance de firma propuesto

Construir el instalador Windows NSIS x64 a partir de un tag público en runners alojados de GitHub Actions, con dependencias fijadas. Solicitar firma del instalador de NIP y, sólo según la configuración aprobada por SignPath, de los binarios propios. No volver a firmar DLL ni ejecutables de terceros como propios. Los binarios upstream conservan sus firmas y licencias.

Tras la aprobación se añadirá la atribución exigida: “Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org)”. Esta frase describe el servicio solicitado, **no un patrocinio ni una firma ya concedidos**.

## Privacidad

NIP no implementa telemetría, analítica ni cuentas centrales. Crear o conectar un servidor inicia conexiones de aplicación; unirse a una llamada habilita audio. El anfitrión recibe IP, nombre visible, presencia y mensajes. En Electron v0.1.2 coordina audio WebRTC directo. En la implementación Rust nativa retransmite también los paquetes Opus y puede escucharlos: el transporte está cifrado por salto, sin E2EE frente al anfitrión. Ver [transición nativa](docs/NATIVE.md) para límites y almacenamiento. Descargar de GitHub y solicitar firma a SignPath implica usar servicios externos con sus propias políticas: [GitHub](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement), [SignPath](https://about.signpath.io/privacy-policy).

## Proceso futuro

La integración se activará sólo tras aprobar la solicitud y recibir organización, proyecto, política de firma y token reales. No se almacenan secretos en el repositorio. El artefacto deberá subirse primero a GitHub Actions; SignPath verificará origen y requerirá aprobación manual. Se publicará el resultado firmado con checksum nuevo, sin etiquetar como firmado el archivo original.

Fuentes oficiales: [condiciones](https://signpath.org/terms.html), [solicitud](https://signpath.org/apply.html), [GitHub como sistema de build confiable](https://docs.signpath.io/trusted-build-systems/github).
