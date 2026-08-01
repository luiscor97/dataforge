# Roadmap 2.0 — Reconstrucción agéntica y salida con significado

**Estado:** Vigente — ejecuta [RFC-0002](../rfcs/RFC-0002-autonomy-ladder.md),
aprobada el 2026-08-01
**Fecha:** 2026-07-29 (actualizado 2026-08-01)
**Relacionada con:** RFC-0001 §45 (roadmap maestro 1.0); RFC-0002 (escalera de
autonomía); ADR-0037 (contratos congelados); ADR-0040..0045

> **Nomenclatura.** M2.1–M2.6 dan la **2.0**, igual que M0.1–M0.9 dieron la 1.0.
> No hay ningún hito intermedio que publique una versión: un run que cumpla la
> definición de hecho con un humano en el gate es el L1 entregable que este
> documento ya contempla como punto de parada legítimo, no una release aparte.

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

**Estado: completo (2026-08-01).** `Actor::Agent` distinguible en el ledger
(ADR-0043 §3); `review decide-batch` atómico con un evento por decisión;
`plan tree`; y los dos crates.

`df-tools` expone 23 herramientas —11 `observe`, 9 `build`, 3 `commit`— y
rechaza cualquier nombre ausente del registro, de modo que una herramienta no
puede existir por ser alcanzable sin estar declarada.
`Capability::requires_authorization` es propiedad de la **clase** y no de la
herramienta: una puerta que enumerase herramientas fallaría abierta el día que
se añade una y nadie actualiza la lista. Hoy no hay puerta —ADR-0041 sigue en
Propuesta—, así que la costura queda declarada y sin efecto.

`df-mcp` habla JSON-RPC 2.0 por stdio, sin red ni estado de sesión. El actor
**no es un parámetro**: todo se atribuye a `Actor::Agent`, porque una
atribución que el llamante elige no es atribución. El protocolo se implementó
directamente, sin SDK — que resuelve la deuda (b) de ADR-0043: cero
dependencias nuevas en un workspace que fija versiones y corre `cargo deny`.

`TOOL_SURFACE_VERSION` y el recuento de herramientas entran en
`frozen_contracts`, lo que obliga a un ciclo de dev-dependency que Cargo
permite justamente para esto.

**Deuda saldada (2026-08-01):** `content_search` y `content_query` completan la
clase `observe` de ADR-0043. La superficie sube a
`dataforge.tool-surface/0.2.0` — aditivo, que es lo que el contrato permite.

`content_query` merece una aclaración, porque ADR-0043 **rechaza exponer SQL
crudo** y a la vez lista esta herramienta como `observe`. Las dos cosas son
ciertas y no se contradicen: esta consulta no alcanza el SQLite del motor —la
fuente de verdad— en absoluto. Corre SQL de solo lectura contra un **snapshot
Parquet derivado**, en un **worker aislado**, sobre una única tabla registrada,
con topes duros de filas, bytes, tamaño de celda, memoria y tiempo. Lo que la
ADR rechaza es una herramienta capaz de rodear los invariantes de la fachada;
esta no puede ni verlos.

### M2.2 — Taxonomía de destino

Que la salida pueda tener bolsas con significado, no solo de procedimiento.
[ADR-0040](../adr/ADR-0040-declared-destination-taxonomy.md), **subsumida en
RFC-0002 como mecanismo del paso 1** (resuelto el 2026-08-01).

- Raíces de destino declaradas y cerradas, en lugar de constantes en un
  `match`.
- `revisar/` como **espejo del árbol de salida**: cada elemento dudoso en su
  mejor ubicación estimada, con el motivo como metadato. Aceptar una revisión
  pasa a ser mover de `revisar/<ruta>` a `output/<ruta>`.
- Hueco neutro `revisar/_sin-ubicar/` y buckets técnicos para fallos, no para
  clasificación.

**Contratos que se mueven:** schema de perfil `1.1.0` → `2.0.0`; migración
append-only para la procedencia de enrutado.

**Estado: mecanismo completo (2026-08-01); dos puntos aplazados con motivo.**

Hecho: las raíces las **declara el perfil** (`destination_roots`), no una
constante, y `DestinationTaxonomy` las lee prestadas — el enrutado corre una
vez por operación y un plan real tiene cientos de miles. `generic` y `legal`
declaran las cuatro raíces que la 1.x llevaba incrustadas, con los mismos
nombres, así que la salida no se mueve ni un byte; el test que lo fija ahora
lee **el perfil que se envía** en vez de una constante, de modo que editar
`profiles/generic/profile.json` lo rompe.

`Profile::load` rechaza *fail-closed* un conjunto de raíces con el que el
planificador no podría enrutar: raíz requerida ausente, id duplicado, dos
raíces sobre una carpeta, o un nombre de carpeta con separador o `..` — este
último es el que importa de verdad, porque colocaría una raíz **fuera del
output root**.

La procedencia de enrutado se persiste en `plan_operations`
(migración 0020, ADR-0040 §3), por **id de raíz y no por nombre de carpeta**,
para que renombrar una carpeta no reescriba la procedencia de los planes
anteriores. `NULL` significa *no registrado*, nunca «raíz activa». No entra en
el manifiesto congelado: va al lado de la operación como evidencia, igual que
`reason`, así que el digest sigue siendo el de 1.x.

**Aplazado, y no por falta de tiempo:** el hueco `revisar/_sin-ubicar/` y los
buckets técnicos. Hoy **nada los escribiría**. `_sin-ubicar/` necesita un caso
sin ubicación estimada, que no existe hasta que haya clasificación (M2.3);
`_ilegible/` y `_verificacion-fallida/` los llena el executor o el verificador
ante un fallo de disco, que es robustez de M2.6. Declarar carpetas que nadie
usa es funcionalidad simulada (CONTRIBUTING) y adelantar hito (regla 7).

La propiedad del espejo, en cambio, **ya se cumple**: una copia a revisión
aterriza en `<raíz de revisión>/<misma ruta que tendría en la salida>` y el
motivo ya viaja como metadato en `reason`, no como carpeta. Aceptar una
revisión ya es mover de `revisar/<ruta>` a `output/<ruta>`. Lo que M2.3 añade
es que esa «misma ruta» deje de ser la del origen y pase a ser la que la
clasificación estime.

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

**Y buena parte de la prueba ya existe.** Medido: 118,9 GB de esos 234,2 GB
están dentro de carpetas que el motor ya ha demostrado que **no tienen nada
propio** — el lado contenido de una relación `TREE_EMBEDDED`, cuyo `CHECK`
exige `unique_files = 0`. Son 708 carpetas maximales y 67.648 archivos. Se
copian igual porque `classify_duplicate_set` mira igualdad de carpeta y no
consulta `tree_relations`.

Conectar esas dos cosas es [ADR-0045](../adr/ADR-0045-embedded-tree-duplicates.md),
y es el primer trabajo de este hito porque desbloquea la mitad de la
redundancia bloqueada sin releer un byte.

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

> Plan de ejecución de este hito y de los dos siguientes —crates, tipos,
> migraciones, contratos y orden— en
> [estructura-m2.4-m2.6.md](estructura-m2.4-m2.6.md).

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
  contestado mejor. **Mitigado el 2026-08-01**: RFC-0002 aprobada, ADR-0040
  subsumida, y el índice de ADR completado con las 0040–0045 que le faltaban. La causa raíz era que el plan vivía en una rama sin fusionar, así
  que quien abría otra rama no lo veía. Estado en
  [estado-superficie-agentica.md](estado-superficie-agentica.md), que sigue
  siendo lo primero que hay que leer al retomar el repo.
- **Calibración de confianza sin datos.** RFC-0002 deja abierto un modo sombra
  que coloque todo en el espejo y compare con la decisión humana para fijar
  umbrales. Sin eso, los umbrales de auto-colocación son una suposición.
- **Suponer que la política de duplicados basta.** Se dio por hecho, en esta
  misma línea de trabajo, que elegir `CONSOLIDATE_ALL` produciría una salida
  deduplicada de unos 204 GB. Medido, el ahorro alcanzable es de 5,45 GB. La
  cifra falsa se sostuvo varias conversaciones porque nadie había ejecutado la
  política contra datos reales. De ahí que M2.3 sea precondición y no adorno,
  y de ahí que la definición de hecho lleve umbrales verificables.
