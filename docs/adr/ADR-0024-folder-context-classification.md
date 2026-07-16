# ADR-0024 — Clasificación de contexto de carpetas por marcadores de perfil

**Estado:** Aceptada
**Fecha:** 2026-07-14
**Relacionada con:** RFC-0001 §18, §15.5, §25.2, §25.4, regla 9; ADR-0026

**Revisada:** 2026-07-16 para reflejar los perfiles declarativos `generic` y
`legal`, su uso real en planificación y el rechazo de ids desconocidos.

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
   canónica; se guarda por carpeta y el análisis la usa al puntuar
   representantes (§15.5) y clasificar conjuntos para la política del plan.

3. **Dos perfiles embebidos y fallo cerrado.** `generic` no declara fronteras
   protegidas (§25.4). `legal` hereda sus marcadores genéricos y añade
   fronteras por nombre para expedientes, procedimientos, asuntos, clientes,
   personas y correspondencia (ADR-0026). Un id desconocido se rechaza al
   crear, abrir y analizar el proyecto: nunca cae silenciosamente a
   `generic`, porque un typo eliminaría las protecciones solicitadas.

4. **La clasificación es evidencia y la política la consume explícitamente.**
   Clasificar una carpeta no borra ni omite nada por sí solo. Sus
   penalizaciones alimentan el representante lógico (ADR-0025), y sus
   fronteras condicionan las políticas de duplicado del plan. La política por
   defecto `REPORT_ONLY` copia todas las apariciones; las políticas de
   consolidación son opt-in y nunca atraviesan una frontera protegida.

5. **Dónde se ejecuta y persistencia.** El cómputo corre dentro del paso
   `analyze` (transición `HASHED → ANALYZING → ANALYZED`), tras las firmas de
   carpeta (ADR-0023). Se persiste en la tabla `folder_contexts` de la
   migración `0007_contexts.sql` (una fila por carpeta: `kind`,
   `is_protected_boundary`, `penalty`, `marker`). El recómputo es idempotente.
   Se emite el evento de auditoría `CONTEXTS_CLASSIFIED`. El informe
   `dataforge report contexts` lista carpetas genéricas por penalización y
   fronteras protegidas con el marcador que las justificó.

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

- DataForge distingue contenedores de bajo valor y fronteras explícitas por
  perfil. Esa evidencia ya guía el representante y las políticas de duplicado
  sin conceder a la clasificación capacidad destructiva.
- El coste es proporcional al número de carpetas del snapshot (una
  comparación de nombre por carpeta), no a los bytes.
- Deuda aceptada: la clasificación sigue basada en nombres exactos o prefijos
  acotados. No interpreta contenido ni demuestra por sí sola que dos carpetas
  pertenezcan al mismo asunto real.
- Condición de revisión: si se incorpora contexto derivado de contenido,
  decidir si `folder_contexts` coexiste con nuevas relaciones o se convierte
  en una proyección de ellas. Hasta entonces no debe presentarse como grafo
  semántico.
