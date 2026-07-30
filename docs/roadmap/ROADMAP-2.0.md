# Roadmap 2.0 — Reconstrucción agéntica y salida con significado

**Estado:** Borrador
**Fecha:** 2026-07-29
**Relacionada con:** RFC-0001 §45 (roadmap maestro 1.0); RFC-0002 (escalera de
autonomía); ADR-0037 (contratos congelados); ADR-0040..0044

## Qué es la 2.0

La 1.0 produce una **copia segura, explicable y verificable**. Inventaría un
origen sin tocarlo, prueba identidad con doble hash, detecta duplicados y
árboles injertados, y materializa una copia bajo manifiesto congelado con
ledger encadenado. Todo eso funciona y está probado hasta 1.000.000 de
archivos.

Lo que la 1.0 **no** hace es decidir qué significa cada archivo. Su salida es
la estructura del origen partida en cuatro bolsas de procedimiento. Un humano
tiene que responder cada elemento de la cola de revisión, y en un archivo real
esa cola son miles.

**La 2.0 cierra esa brecha:** el motor produce una salida ordenada y sin
duplicados por sí mismo, conducido por un agente que propone y por reglas
deterministas que autorizan.

La tesis operativa es la de [RFC-0002](../rfcs/RFC-0002-autonomy-ladder.md), y
no se negocia: **la IA nunca es la autoridad**. Propone clasificación y
estructura; una regla declarativa verifica sobre evidencia recalculada
localmente; lo que no se sabe manejar va a `revisar/` y el run no se detiene.

## Por qué es un salto mayor y no una 1.1

`destination_relative_path` participa en la clave de idempotencia y en el
manifiesto congelado bajo SHA-256. Enriquecer la salida obliga a cambiar cómo
se calcula, y [ADR-0037](../adr/ADR-0037-frozen-contracts.md) congela ese
contrato para toda la 1.x, con un único mecanismo de cambio: **subir versión y
escribir el ADR en el mismo commit, nunca editar in place**.

La 2.0 es ese vehículo. Es la razón de que exista, no un efecto secundario.

## Garantías que no cambian

La autonomía cambia **quién autoriza**, nunca **qué garantiza el motor**. Se
mantienen intactas, y cualquier propuesta que las toque se rechaza:

1. El origen es inmutable. No hay código que escriba, renombre o borre dentro
   de un source root.
2. No existe borrado ni sobrescritura de archivos de usuario.
3. SQLite es la única fuente de verdad.
4. Todo cambio de estado pasa por la máquina de estados y deja evento en el
   ledger encadenado, en la misma transacción.
5. Clientes solo hablan con `df-facade`.
6. El verificador re-lee y re-hashea sin fiarse ni del agente ni del executor.

El radio de daño de un agente totalmente autónomo sigue siendo una
**reconstrucción subóptima**: reversible borrando el árbol de salida, con los
orígenes intactos.

## Hitos

El orden es obligado: cada hito depende de que exista el anterior.

### M2.1 — Superficie agéntica

Que un agente pueda conducir el motor sin acoplarse a la ABI de Rust.
[ADR-0043](../adr/ADR-0043-facade-tool-surface.md), paso 1 de RFC-0002.

- `df-tools`: adaptador tipado sobre la fachada, con tres clases de capacidad
  — `observe` (solo lectura, libre), `build` (avanza análisis y plan, no
  copia), `commit` (cambia estado real, y es lo único que pasará por
  autorización).
- `df-mcp`: servidor MCP por stdio, sin red. Expone **solo** el vocabulario de
  la fachada: ni FS arbitrario, ni SQL crudo, ni shell.
- Nombres y esquemas entran en `frozen_contracts`; cambios solo aditivos.

**Hecho ya:** `Actor::Agent` distinguible en el ledger (ADR-0043 §3);
`review decide-batch` atómico con un evento por decisión; `plan tree`.

**Pendiente:** los dos crates. La CLI **no** sustituye la superficie: un
agente con la CLI tiene el shell, que es exactamente lo que ADR-0043 evita
poniendo el vocabulario en la frontera de transporte.

### M2.2 — Taxonomía de destino

Que la salida pueda tener bolsas con significado, no solo de procedimiento.
[ADR-0040](../adr/ADR-0040-declared-destination-taxonomy.md), pendiente de
subsumirse en RFC-0002.

- Raíces de destino declaradas y cerradas, en lugar de constantes en un
  `match`.
- `revisar/` como **espejo del árbol de salida**: cada elemento dudoso en su
  mejor ubicación estimada, con el motivo como metadato. Aceptar una revisión
  pasa a ser mover de `revisar/<ruta>` a `output/<ruta>`.
- Hueco neutro `revisar/_sin-ubicar/` y buckets técnicos para fallos, no para
  clasificación.

**Contratos que se mueven:** schema de perfil `1.1.0` → `2.0.0`; migración
append-only para la procedencia de enrutado.

**Hecho ya:** `DestinationTaxonomy`, preservando la salida 1.x byte a byte con
test que lo fija.

### M2.3 — Clasificación

Que el motor distinga qué es cada archivo, no solo en qué estado está.

**Es la precondición de la deduplicación, no una mejora de orden.** Medido
sobre el corpus real: de 28.537 conjuntos de duplicados, solo **625** tienen
todas sus copias en una misma carpeta. `classify_duplicate_set` solo puede
afirmar `WithinSameContext` en ese caso; el resto queda `UnknownContext`, y
§15.2 prohíbe inferir redundancia, así que **ninguna política lo consolida —
tampoco `CONSOLIDATE_ALL`**.

| | |
| --- | --- |
| Redundancia total | 239,7 GB |
| Alcanzable por cualquier política de la 1.0 | **5,45 GB** |
| Bloqueada por falta de clasificación | **234,2 GB** |

Con perfil `generic` sobre ese archivo, 36.381 de 36.459 carpetas quedan
`NEUTRAL` y ninguna es frontera protegida. Sin clasificación de contexto no
hay deduplicación posible, por diseño y con razón: el motor se niega a suponer
que una copia que está en otro sitio sobra.

Fijado por `crates/df-planner/tests/consolidation_savings.rs`.

- Recuperación canónica (Modo 1 de RFC-0002): dedup por contenido eligiendo
  representante, y auditoría de árboles injertados. La mayoría se resuelve
  **sin IA**.
- Taxonomía inventada (Modo 2) cuando no hay árbol recuperable, explicada en
  el informe.
- Conectar la evidencia de `df-media`, que ya extrae duración y resolución en
  worker aislado, a la clasificación: un curso de seis horas es material
  profesional; una serie de seis episodios no. Hoy esa evidencia existe y no
  alimenta ninguna decisión.
- Reglas duras heredadas del caso real: nunca deduplicar por nombre; evidencia
  compartida entre asuntos siempre a revisión; origen suelto se marca, no se
  colapsa.

### M2.4 — `df-rules`

La autoridad determinista del gate. [ADR-0041], paso 2 de RFC-0002, y el ~70%
del trabajo que hace segura la retirada del humano.

- Devuelve `Autorizar | A-revisar | Denegar` **con el id de la regla que lo
  determinó**.
- Fronteras duras *fail-closed*: nunca fusionar proyectos, nunca tocar
  orígenes protegidos, destino vacío obligatorio, presupuestos no superables.
- Reglas versionadas y con checksum, misma disciplina que las migraciones.
  Digest del conjunto sellado en cada decisión.

### M2.5 — Consentimiento por política

[ADR-0042], extensión de ADR-0034. El humano aprueba **una vez** una política
de divulgación con presupuesto de llamadas, tokens y gasto. Agotado el
presupuesto, lo ambiguo restante va a `revisar/`: degrada, no bloquea, no
dispara la factura. La clave sigue en el almacén de credenciales del sistema.

### M2.6 — `df-agent`

[ADR-0044]. El bucle completo: intención → plan → reglas → congelar → ejecutar
→ verificar → informe. Con presupuestos, cortacircuitos por tasa de
ambigüedad, modo dry-run y mapa origen→destino exportable.

Robustez de disco viejo como requisito duro, no como mejora: errores de
lectura a `revisar/_ilegible/` sin abortar; pre-vuelo de espacio antes de
copiar; reanudación exacta desde el manifiesto; fallo de verificación a
`revisar/_verificacion-fallida/` con el run continuando.

## Definición de hecho

La 2.0 no se declara terminada por tener los crates. Se declara terminada
cuando **reproduce por sí sola, sobre el corpus real, el resultado que hoy solo
se alcanzó con scripts y criterio humano a lo largo de diez días**:

| Criterio | Umbral |
| --- | --- |
| Origen modificado | Ninguno, verificado |
| Duplicados exactos activos en la salida | 0 grupos |
| Cobertura | Ningún contenido del origen sin representación ni motivo |
| Basura técnica en zonas firmes | 0 (`~$*`, `~WRL*.tmp`, `Thumbs.db`, `desktop.ini`) |
| Ocio en zonas firmes | 0 |
| Rutas raíz repetidas en zonas firmes | 0 |
| Verificación independiente | `COMPLETED` |
| Ledger | Cadena verificada, con procedencia por decisión |
| Mapa origen→destino | Completo y exportable |

Referencia de escala: 158.219 archivos, 443,9 GB, 28.537 conjuntos de
duplicados, 239,7 GB redundantes. La cola de revisión tiene 5.334 elementos y
3.702 son la misma clase, que es el dato que justifica decidir por clase y no
por elemento.

Un run que cumpla esa tabla **sin intervención humana durante la fase larga**
es la 2.0. Uno que la cumpla con un humano en el gate es un L1 entregable, y
es un punto de parada legítimo si la 2.0 se alarga.

## Mecánica de versión

- Las versiones de `Cargo.toml` y `package.json` suben **al publicar**, no al
  empezar. Mientras se desarrolla, la rama es la que identifica el trabajo.
- Cada ADR que mueva un contrato actualiza la expectativa de
  `frozen_contracts` **en su mismo commit**, según ADR-0037 §2. Un test roto
  sin ADR es un accidente que revertir.
- Las migraciones siguen siendo append-only y numeradas a continuación de la
  última aplicada (0019).

## Compatibilidad

Puramente aditiva hacia atrás en lo que importa: un proyecto creado con 1.x y
abierto con 2.0 conserva sus planes, porque las rutas de destino ya están
materializadas como cadenas y un manifiesto aprobado es inmutable por
construcción. No se reescribe historia. El perfil `generic` mantiene la salida
1.x byte a byte, así que quien no adopte un perfil nuevo no ve cambios.

## Riesgos

- **La clasificación es donde se equivoca un agente.** Se mitiga con el
  vocabulario acotado (solo copiar y anotar), el fallback universal a
  `revisar/` y la reversibilidad total. El peor resultado es una copia mal
  organizada, nunca una pérdida.
- **Trabajo en paralelo divergiendo.** Ya pasó: `Actor::Agent` se implementó
  dos veces, y ADR-0040 respondió una pregunta que RFC-0002 ya había
  contestado mejor. Estado en
  [estado-superficie-agentica.md](estado-superficie-agentica.md); RFC-0002
  debería fusionarse antes de seguir.
- **Calibración de confianza sin datos.** RFC-0002 deja abierto un modo sombra
  que coloque todo en el espejo y compare con la decisión humana para fijar
  umbrales. Sin eso, los umbrales de auto-colocación son una suposición.
- **Suponer que la política de duplicados basta.** Se dio por hecho, en esta
  misma línea de trabajo, que elegir `CONSOLIDATE_ALL` produciría una salida
  deduplicada de unos 204 GB. Medido, el ahorro alcanzable es de 5,45 GB. La
  cifra falsa se sostuvo varias conversaciones porque nadie había ejecutado la
  política contra datos reales. De ahí que M2.3 sea precondición y no adorno,
  y de ahí que la definición de hecho lleve umbrales verificables.
