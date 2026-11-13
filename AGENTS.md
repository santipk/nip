# Instrucciones para agentes

Respuestas en español, directas y orientadas a implementación, para un usuario con conocimientos técnicos intermedios. No inventar información; declarar incertidumbres. Cambios mínimos, sin abstracciones ni funcionalidades especulativas.

Para cualquier tarea técnica o de programación, activar y seguir la skill global `karpathy-guidelines` como regla base obligatoria. Si no está disponible, informarlo sin fingir que se leyó.

## Agent skills

Las skills originales de Matt Pocock están en `.agents/skills/`. Consultar `docs/agents/skills.md` para procedencia y flujo. Instalar una skill no equivale a ejecutarla. Leer su `SKILL.md` antes de usarla y respetar los alcances ya autorizados por el usuario.

### Issue tracker

Markdown local por defecto; ver `docs/agents/issue-tracker.md`. No publicar conversaciones, especificaciones o tickets en servicios externos sin pedido explícito.

### Domain docs

Un único contexto, con glosario y ADRs creados según necesidad. Ver `docs/agents/domain.md`.

## Verificación

Ejecutar `npm run typecheck`, `npm test` y `npm run build` para cambios que lo requieran. Las pruebas de escritorio/audio y del paquete son adicionales según el alcance; no presentar pruebas sintéticas como validación de micrófonos físicos o conectividad WAN.
