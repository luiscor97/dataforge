# ADR-0043 — Superficie de herramientas sobre la fachada (base agéntica)

**Estado:** Propuesta
**Fecha:** 2026-07-21
**Relacionada con:** RFC-0002 (§ componentes nuevos, paso 1); RFC-0001 §23
(clientes solo vía fachada); ADR-0034 (IA asistida)

> Numeración de ADR reservada por RFC-0002 (0041 `df-rules`, 0042
> consentimiento-por-política, 0043 superficie de herramientas, 0044
> `df-agent`). El número final se fija al commitear, para no colisionar con el
> trabajo en paralelo de M1.0.1.

## Contexto

`df-facade` ya expone el pipeline completo como **funciones sin estado**,
identificadas por `project_dir` y `Actor`, con toda la mutación de estado
persistida en SQLite (única fuente de verdad):

- Inventario/análisis (no copian nada): `scan_project`, `hash_project`,
  `analyze_project`, `analyze_similarity`, `analyze_media`,
  `extract_project_content`, `build_content_artifacts`, `create_plan`,
  `validate_plan`.
- Cambio de estado real (el gate): `approve_plan` (congela el manifiesto,
  §26.4), `execute_plan` (copia), `verify_project_output` (re-lee y re-hashea).
- Solo lectura: `project_status`, `duplicate_report`, `tree_clone_report`,
  `context_report`, `structural_anomaly_report`, `structural_review_queue`,
  `similarity_report`, `verify_audit`, `query_project_content`,
  `search_project_content`.
- Revisión existente: `structural_review_queue` + `decide_structural_review`, y
  la explicación de items por IA `ai_explain_review` (M0.7).

Hoy el único cliente que consume esto es la CLI, enlazando `df-facade`
directamente (ABI de Rust). Para que un **agente** —empotrado o externo, con el
**proveedor de IA que sea**— pueda *usar* la herramienta hace falta una
superficie **tipada, estable, versionada y acotada** que no exista solo como
símbolos de Rust. Sin ella no hay forma de conducir el pipeline de forma
agéntica ni de garantizar, a nivel de transporte, que el agente no puede hacer
nada fuera del vocabulario del motor.

## Decisión

1. **Crate nuevo `df-tools`: adaptador tipado y estable sobre la fachada.**
   Envuelve las funciones públicas de `df-facade` en tres **clases de
   capacidad** explícitas:
   - **`observe`** (solo lectura): reports, status, colas, consultas. Libremente
     invocables por el agente.
   - **`build`** (avanzan inventario/análisis/plan; **no copian**): scan, hash,
     analyze*, extract, create_plan, validate_plan. Reversibles y sin efecto en
     el destino.
   - **`commit`** (cambio de estado real): `approve_plan`, `execute_plan`,
     `verify_project_output`. **Son las únicas** que, cuando exista `df-rules`
     (ADR-0041), pasan por autorización. Esta separación deja "mirar y pensar"
     libre y "cambiar estado" bajo puerta.

2. **Binario nuevo `tools/df-mcp`: servidor MCP** que enlaza `df-tools` y expone
   cada herramienta con esquema JSON de entrada/salida. Así **cualquier runtime
   de agente / proveedor** conduce DataForge sin acoplarse a la ABI de Rust. El
   servidor **no expone nada fuera del vocabulario de la fachada**: no hay
   herramienta de FS arbitrario, ni de SQL crudo, ni de shell. El principio de
   *vocabulario de acciones acotado* (RFC-0002) queda **forzado en la frontera
   de transporte**, no confiado al buen comportamiento del modelo. Transporte
   local (stdio) por defecto; sin red.

3. **Nuevo `Actor::Agent`** en `df-domain::event::Actor` (hoy
   `System | Cli | Desktop | Test`). Adición compatible: `as_str`/`parse`
   ganan `"agent"`. Toda acción autónoma queda **atribuida y distinguible** de
   la conducción humana en el ledger encadenado; la procedencia extendida (regla,
   política, confianza, proveedor) la sella el gate autónomo (RFC-0002), no este
   ADR.

4. **La superficie es contrato congelado.** Nombres de herramienta y esquemas de
   entrada/salida entran en el test `df-facade::frozen_contracts`, versionados:
   cambios **solo aditivos**; modificar una herramienta es subir versión, nunca
   in place. Los agentes externos dependen de esta estabilidad.

5. **Sin estado oculto.** Cada herramienta se identifica por `project_dir`; no
   hay sesión ni estado en memoria del servidor. El estado completo vive en
   SQLite, así que un agente puede caerse y reanudar sin coordinación especial.

## Alternativas consideradas

- **Que el agente externo enlace `df-facade` directamente** — descartado: atado
  a la ABI de Rust, no agnóstico de lenguaje/proveedor, y sin frontera que
  acote el vocabulario.
- **Una herramienta genérica "ejecuta comando"** — descartado: rompe el
  vocabulario acotado; superficie insegura.
- **Exponer SQL crudo de solo lectura** — descartado: saltaría los invariantes
  de la fachada; las herramientas `query_project_content`/reports son la vía
  segura y con alcance.

## Consecuencias

- **Habilita L1 de inmediato**: el humano mantiene el gate y el agente propone
  vía herramientas. L2 se añade cuando `df-rules` ocupe el gate `commit`.
- El servidor MCP es **el punto de imposición** del vocabulario acotado y de la
  separación observe/build/commit.
- **Deuda declarada**: (a) *hueco de propuesta de plan* — `create_plan` toma hoy
  `DuplicatePolicy`, no decisiones de colocación/`revisar`; el input de
  colocación autónoma es trabajo de `df-agent`/`df-rules` (ADR-0044/0041), no de
  este ADR; la superficie es agnóstica a la colocación. (b) La dependencia del
  servidor MCP debe pasar `cargo deny`/`cargo audit`; se elegirá una mínima o se
  implementará el protocolo directamente sobre stdio.
- **Revisar si**: el protocolo MCP cambia de forma incompatible, o se necesita
  transporte de red (que exigiría su propio ADR de autenticación y superficie).
