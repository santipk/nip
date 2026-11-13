# Issue tracker

Por defecto, Markdown local: especificaciones en `.scratch/<feature>/spec.md` y un archivo por ticket en `.scratch/<feature>/issues/<NN>-<slug>.md`. Los tickets registran criterios de aceptación, dependencias y estado (`ready-for-agent`, `in-progress`, `done`). `.scratch/` se ignora para evitar publicar contexto privado por accidente.

Repositorio público: https://github.com/santipk/nip. Usar GitHub Issues como tracker requiere un pedido explícito; la autorización para publicar el código no autoriza publicar conversaciones o especificaciones. No crear issues ni etiquetas durante la instalación de skills. `to-spec` escribe en el tracker local configurado; no llama a servicios externos por defecto. No hay triage automático ni PRs como superficie de pedidos.
