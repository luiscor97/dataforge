# ADR-0024 — Clasificación de contexto de carpetas por marcadores de perfil

**Estado:** Aceptada
**Fecha:** 2026-07-14
**Relacionada con:** RFC-0001 §18, §15.5, §25.2, §25.4, regla 9

## Contexto

RFC-0001 §18 describe un grafo contextual rico —anclas fuertes, señales
ponderadas, propagación acotada— para reconstruir asuntos, clientes y
expedientes. Esa maquinaria completa (extracción de entidades, Message-ID,
números de procedimiento) es grande y depende de extractores que aún no
existen. Pero una parte del §18 es puramente estructural y determinista: el
§18.3 penaliza carpetas genéricas (Descargas, Escritorio, Backup,
Recuperado, carpeta genérica) al elegir el representante lógico de un
duplicado (§15.5), y el §9.6 marca fronteras protegidas que la deduplicación
no debe disolver (regla 9). El segundo incremento de Milestone 0.2 entrega
esa parte determinista sin adelantar la extracción de entidades.

## Decisión(es)

1. **Clasificación por marcadores de nombre, determinista.** Cada carpeta del
   snapshot se etiqueta como `GENERIC`, `PROTECTED` o `NEUTRAL` comparando su
   `normalized_name` (minúsculas) con un conjunto de marcadores del perfil
   activo. No hay inferencia estadística ni lectura de contenido: el mismo
   inventario produce siempre la misma clasificación.

2. **Marcadores genéricos y penalizaciones del §18.3.** El perfil `generic`
   reconoce como contenedores de bajo valor: `descargas`/`downloads` (50),
   `escritorio`/`desktop` (45), `backup`/`copia de seguridad`/`respaldo`
   (40), `recuperado`/`recovered` (35) y `temp`/`temporal`/`copia`/`nueva
   carpeta` y patrones de copia (`* - copia`, `copia de *`) (30). La
   penalización es el peso con que §18.3 degrada una carpeta como ubicación
   canónica; se guarda por carpeta para que una fase posterior de política de
   duplicados la use al puntuar representantes (§15.5).

3. **Perfil `generic` conservador, sin marcadores protegidos (§25.4).** El
   perfil genérico no intenta inferir sectores, así que no declara
   marcadores protegidos: `is_protected_boundary` es siempre falso bajo él.
   La estructura de datos y el tipo `ContextKind::Protected` existen ya para
   que un perfil jurídico (expediente, pericial, cliente, asunto) pueda
   declarar fronteras protegidas sin cambiar el esquema; ese perfil se añade
   en una rebanada posterior. Un perfil desconocido cae a `generic`.

4. **Solo evidencia, no acción.** La clasificación baja el ranking de una
   carpeta como ubicación representativa, pero no marca ningún archivo para
   eliminación ni genera operaciones de plan. La consolidación de duplicados
   consciente del contexto —usar estas penalizaciones para elegir qué copia
   preservar— es una rebanada futura que además depende de la política de
   duplicados (§15.4).

5. **Dónde se ejecuta y persistencia.** El cómputo corre dentro del paso
   `analyze` (transición `HASHED → ANALYZING → ANALYZED`), tras las firmas de
   carpeta (ADR-0023). Se persiste en la tabla `folder_contexts` de la
   migración `0005_contexts.sql` (una fila por carpeta: `kind`,
   `is_protected_boundary`, `penalty`, `marker`). El recómputo es idempotente.
   Se emite el evento de auditoría `CONTEXTS_CLASSIFIED`. Un nuevo informe de
   CLI, `dataforge report contexts`, lista las carpetas genéricas por
   penalización descendente.

## Alternativas consideradas

- **Implementar ya el grafo contextual completo del §18** (entidades,
  anclas, propagación) — descartada por alcance y dependencias: requiere
  extractores de contenido (§0.4) que no existen; la parte por marcadores
  aporta valor inmediato para el ranking de representantes sin ellos.
- **Coincidencia por subcadena en cualquier parte del nombre** (p. ej.
  marcar `backups_de_juan` como genérica) — descartada: genera falsos
  positivos que degradarían carpetas legítimas; se usa igualdad de nombre
  normalizado más un puñado de patrones de copia bien acotados.
- **Reutilizar ya las tablas `contexts` / `context_memberships` del §10.1** —
  descartada de momento: esas tablas modelan nodos de contexto con jerarquía
  y pertenencia para el grafo completo; una tabla `folder_contexts` dedicada
  es honesta sobre el alcance de esta rebanada y no ocupa prematuramente ese
  nombre. Se revisará cuando exista el grafo.
- **Marcar fronteras protegidas por heurística de profundidad** (p. ej. toda
  carpeta de primer nivel) — descartada por arbitraria e insegura; las
  fronteras protegidas deben venir de marcadores explícitos de un perfil.

## Consecuencias

- DataForge distingue ya carpetas de bajo valor de materias reales, cimiento
  para "diferenciar repetición legítima" y para las políticas de duplicado
  conscientes del contexto de Milestone 0.2, manteniendo la garantía de "solo
  evidencia".
- El coste es proporcional al número de carpetas del snapshot (una
  comparación de nombre por carpeta), no a los bytes.
- Deuda aceptada, a registrar en el backlog de Milestone 0.2: el perfil
  jurídico con marcadores protegidos; la extracción de entidades y el grafo
  contextual completo (§18.2–§18.4); y el uso de las penalizaciones en la
  puntuación del representante lógico (§15.5) dentro de la política de
  duplicados. Nada de esto existe todavía ni se insinúa en el plan.
- Condición de revisión: cuando llegue el perfil jurídico y el grafo de
  entidades, revisar si `folder_contexts` se fusiona con las tablas
  `contexts`/`context_memberships` o coexiste con ellas.
