# ADR-0028 — Reglas declarativas, anomalías y revisión humana

**Estado:** Aceptada
**Fecha:** 2026-07-16
**Relacionada con:** RFC-0001 §12.5–§12.7, §25, regla 9; ADR-0026, ADR-0027

## Contexto

M0.2 necesita distinguir archivos temporales, metadatos del sistema y casos
ambiguos sin convertir heurísticas en permisos destructivos. También necesita
hacer visibles anomalías estructurales que un plan de copia no debe resolver
en silencio. Codificar esos criterios directamente en el plan impediría
versionarlos, explicar por qué coincidieron y conservar las decisiones humanas
sin reescribir evidencia anterior.

## Decisión

1. **Las reglas forman parte del perfil declarativo.** El esquema de perfil
   `1.1.0` añade una lista ordenada de reglas con id, versión, patrón,
   clasificación, confianza, acción y riesgo. Los perfiles siguen embebidos en
   el binario y validados al cargar; una regla inválida hace fallar el perfil.

2. **El predicado de M0.2 es deliberadamente pequeño.** Solo se admite un glob
   case-insensitive sobre el nombre de archivo: `*` representa cero o más
   caracteres y `?`, uno. Se rechazan separadores de ruta. Gana la primera
   regla que coincide, por lo que la precedencia es explícita y estable.

3. **El conjunto de acciones es cerrado y conservador.** Una regla o una
   decisión humana solo puede seleccionar `COPY_ACTIVE`, `COPY_REVIEW`,
   `COPY_SEPARATED` o `COPY_TEMPORARY`. No existe acción declarativa de borrar,
   sobrescribir o modificar el origen. Una frontera protegida sigue
   prevaleciendo sobre cualquier recomendación.

4. **Cada coincidencia conserva su evidencia.** El motor persiste id de regla,
   versión, aparición, clasificación, confianza, riesgo y JSON canónico con el
   nombre y patrón que produjeron la coincidencia. Los ids derivados son
   SHA-256 estables sobre campos delimitados; reanalizar el mismo snapshot usa
   `INSERT OR IGNORE` y no duplica hallazgos.

5. **El detector de anomalías de M0.2 es finito y determinista.** Materializa:

   - mismo nombre normalizado y carpeta, pero contenidos exactos distintos;
   - identidad de ruta con representación visual degradada, conservando la
     identidad raw;
   - archivos o carpetas no leídos normalmente;
   - rutas de al menos 240 unidades UTF-16;
   - árboles parciales con contenido exclusivo en ambos lados;
   - árboles embebidos que todavía deben preservarse hasta revisión.

   Cada anomalía lleva tipo, severidad, resumen y JSON canónico autocontenido.
   Una anomalía no afirma corrupción ni autoriza una acción destructiva.

6. **La revisión es append-only.** Las reglas `COPY_REVIEW`, las reglas de
   riesgo alto y las anomalías ambiguas crean `review_items` estables. Las
   decisiones se añaden a `review_decisions`, exigen una justificación no
   vacía y nunca modifican ni eliminan la evidencia automática. La decisión
   más reciente gobierna planes posteriores, conservando el historial entero.

7. **Una revisión pendiente conserva.** Para una aparición pendiente, el
   plan emite `COPY_REVIEW` con confianza cero en vez de omitirla. Una decisión
   resuelta puede moverla a otra de las cuatro operaciones de copia seguras.
   Las recomendaciones de reglas/revisión prevalecen sobre la consolidación de
   duplicados, salvo la protección de regla 9, para que la ambigüedad nunca se
   resuelva perdiendo una copia.

8. **Persistencia.** `rule_matches`, `structural_anomalies`, `review_items` y
   `review_decisions` se añaden en `0010_structural_review.sql` con triggers
   append-only. Los eventos `RULES_EVALUATED`, `ANOMALIES_DETECTED` y
   `REVIEW_DECIDED` enlazan la evidencia con el ledger.

## Alternativas consideradas

- **Reglas Rust compiladas dentro del planner** — descartadas: ocultan
  conocimiento de dominio y dificultan versionar la evidencia.
- **Expresiones regulares o reglas sobre rutas completas** — descartadas para
  M0.2: amplían mucho la superficie de errores y permiten coincidencias menos
  auditables. El glob de nombre cubre los casos entregados.
- **Permitir `DELETE` o `SKIP` en el perfil** — descartado: una heurística de
  metadatos no debe concederse a sí misma capacidad destructiva.
- **Actualizar un único veredicto humano** — descartado: borraría el historial
  de revisión y rompería la trazabilidad.
- **Bloquear todo el plan mientras haya pendientes** — descartado: copiar a
  revisión es una salida conservadora y permite inspeccionar un plan completo
  sin perder cobertura.

## Consecuencias

- El mismo snapshot, perfil y conjunto de decisiones producen la misma guía
  operativa, y cada desviación humana queda explicada.
- Los perfiles `generic` y `legal` incluyen reglas concretas para temporales,
  backups que requieren revisión, metadatos de sistema y correo `.eml`.
- La detección se limita a metadatos estructurales. No extrae entidades, no
  interpreta contenido y no construye relaciones semánticas.
- Los informes y la cola pueden contener falsos positivos; por diseño se
  traducen en conservación/revisión, no en pérdida automática.
- Condición de revisión: ampliar predicados o acciones requiere otra ADR y una
  prueba de que el nuevo lenguaje sigue sin poder escribir en el origen ni
  eludir fronteras protegidas.
