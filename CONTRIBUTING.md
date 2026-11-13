# Contribuir a NIP

Usá `npm ci` con Node.js 22.12 o superior. Antes de enviar cambios ejecutá `npm run typecheck`, `npm test` y `npm run build`. Para cambios de escritorio o audio, ejecutá además las pruebas correspondientes del README y documentá qué verificaste con hardware o redes reales.

Para funcionalidades nuevas, acordá primero el problema, alcance, criterios de aceptación e interfaces de prueba. Usá las skills de `.agents/skills/`: `grill-with-docs` → `to-spec` → `to-tickets` → `implement`; `tdd` guía las pruebas y `code-review` revisa el resultado. Para arreglos pequeños alcanza una descripción concreta y una verificación proporcional. No agregues arquitectura ni funcionalidades especulativas.

Las especificaciones y tickets se guardan por defecto como Markdown local en `.scratch/<feature>/`, ignorado por Git. Si deben revisarse públicamente, publicalos en GitHub Issues sólo por pedido explícito y revisá antes que no contengan datos privados. No incluyas invitaciones reales, perfiles, claves privadas, archivos `.env`, dependencias ni compilados en commits.

La configuración de trabajo de las skills está en `docs/agents/`; son convenciones del proyecto, no especificaciones de producto. Las skills están fijadas a una versión y no se actualizan automáticamente.
