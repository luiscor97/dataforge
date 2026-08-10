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

> **Actualización del 2026-08-10.** Contrastar la superficie contra lo que el
> trabajo original hizo de verdad reabrió deuda en M2.1 y añadió trabajo
> derivado de evidencia a M2.3 y M2.6. El análisis que lo sostiene está en
> [superficie-derivada-del-trabajo-real.md](superficie-derivada-del-trabajo-real.md);
> las secciones fechadas de cada hito son su resultado.

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

`df-tools` expone 25 herramientas —13 `observe`, 9 `build`, 3 `commit`— y
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

#### Deuda reabierta (2026-08-10): el vocabulario está completo, la conducción no

Contrastar las 25 herramientas contra lo que el trabajo original **hizo de
verdad** deja una conclusión incómoda: un agente con esta superficie no puede
terminar la tarea, y no por falta de criterio. Análisis completo en
[superficie-derivada-del-trabajo-real.md](superficie-derivada-del-trabajo-real.md).

El objetivo declarado de este hito era *«que un agente pueda conducir el motor»*.
Se cumplió como **vocabulario** —las 25 herramientas existen, están tipadas y
clasificadas— y no como **conducción** de un trabajo de horas. Son tres huecos
mecánicos, y ninguno necesita inteligencia:

1. **Arrancar y esperar están acoplados.** `hash_project` sobre 443 GB ocupa la
   única sesión stdio durante horas y no devuelve nada hasta terminar. El
   matiz que importa: **los contadores ya existen**. `inventory_summary`
   (`crates/df-db/src/inventory.rs`) publica `hash_done`/`hash_pending` y
   `project_status` ya los expone. No falta telemetría; falta un
   `job_start(stage)` que devuelva un id y un `job_status(id)` que lea lo que
   ya se escribe. La transcripción del trabajo original está llena de «sigue
   vivo» y «va por 30/75»: el operador humano necesitaba ese latido y lo
   obtenía mirando la consola, que es justo lo que un agente no tiene.
2. ~~**Los informes no caben en ninguna ventana de contexto.**~~
   **Resuelto (2026-08-10).** `duplicate_report` devolvía 28.537 conjuntos y
   `structural_review_queue` 5.334 elementos; ahora los seis informes con
   detalle devuelven una **ventana** con `limit`/`offset`, 50 por defecto y un
   techo de 1.000 que el llamante no puede subir. Tres propiedades lo hacen
   fiable: **los totales nunca se acotan** —`redundant_bytes` significa lo
   mismo con un conjunto que con mil, porque los informes ya calculaban los
   escalares aparte del vector—; **la truncación siempre es visible**, en
   `pages.<colección>.has_more`, porque una truncación que no se detecta no es
   paginación sino una respuesta incorrecta con aspecto de correcta; y la lista
   de colecciones tiene **una sola definición**, que leen tanto el dispatch
   como el esquema MCP, de modo que no pueden discrepar. Superficie
   `dataforge.tool-surface/0.3.0`, con `frozen_contracts` actualizado en el
   mismo commit (ADR-0037 §2).
3. **Nada registra si un run sigue vivo.** Buscado en todo `crates/`: ni PID,
   ni host, ni latido, ni marca de actividad. No está a medias, no hay nada.
   Es la causa de que el proyecto de la prueba de la 1.0 siga clavado en
   `EXECUTING` sin que nadie pueda distinguir «hay algo copiando» de «murió
   hace tres días». Es precondición de la autonomía, no robustez opcional:
   **sin esto, reanudar es apostar.**

Estos tres van **antes** que cualquier trabajo de clasificación. No por
prelación de diseño, sino porque sin ellos no hay bucle que optimizar: hay una
llamada que no vuelve.

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

#### Dos informes que la evidencia pidió (2026-08-10)

Derivados del trabajo original, no de principios. Los dos son **lectura sobre
evidencia que ya está en la base**: baratos, y desbloquean el grueso del hito.

**`grafted_tree_report`. Implementado el 2026-08-10.** `tree_relation_report`
da las relaciones, pero el trabajo original necesitó algo más fino: para cada
archivo dentro de un injerto, **su ruta canónica probable** y en cuál de estos
cuatro casos cae. Cifras de la transcripción, sobre 135.378 archivos en 124
prefijos:

| Caso | Archivos | |
| --- | ---: | --- |
| En su ruta canónica, mismo hash | 130.165 | 96,1 % → automático |
| El contenido existe fuera del injerto | 3.977 | 2,9 % → automático |
| **Contenido único dentro del injerto** | **817** | 0,6 % → revisión |
| **Misma ruta canónica, contenido distinto** | **419** | 0,3 % → revisión |

**99,1 % automático / 0,9 % a revisión.** Ese es el umbral de auto-colocación
para árboles injertados y **no es una suposición**: es el reparto medido. Los
dos casos que van a revisión son exactamente los que un humano tiene que mirar,
y son 1.236 elementos, no 135.378.

Reejecutado con la implementación del motor sobre el proyecto de la prueba de
la 1.0 —otra derivación de los prefijos, otro estado del corpus— sale
**97,9 % / 2,1 %** sobre 87.667 archivos en 20 prefijos. La afirmación robusta
no es la cifra exacta sino su orden: **alrededor del 98 % se coloca solo**, y
todo el valor del informe está en el 2 % restante.

Dos cosas que costó aprender, ambas equivocándose primero:

- Un archivo pertenece a **su prefijo más largo** y se cuenta una vez. Los
  injertos anidan, y contar un archivo una vez por cada prefijo bajo el que
  cae daba más archivos que los que tiene el snapshot.
- Las rutas se comparan **por componentes**, no rebanando bytes: minuscular
  una ruta no ASCII puede cambiar su longitud en bytes, y cortar la cadena
  minusculada por una longitud medida en la original es un `panic` esperando
  al nombre de archivo adecuado.

**`name_collision_report`. Implementado el 2026-08-10.** El caso que ninguna
regla de contenido detecta: 106 nombres con contenido distinto entre asuntos,
el peor `00000001.JPG` con **19 hashes en 6 periciales**. El motor ya se negaba
a deduplicar por nombre; lo que faltaba era **poder demostrar por qué**, que es
lo que convierte una negativa en una garantía para quien recibe la entrega.

Agrupa por **contenido distinto**, no por nombre repetido: dos archivos con el
mismo nombre y los mismos bytes son un duplicado, no una colisión, y contarlos
inflaría el hallazgo justo donde tiene que ser preciso. Hay un test con el caso
real —el mismo exhibit numerado en tres periciales, más un duplicado legítimo
que no debe aparecer— y agrupar por nombre lo rompe.

Lee hashes y nombres, que la etapa de hash ya selló, así que **no exige
análisis completo** como los informes estructurales: la colisión importa
sobre todo *antes* de que nadie planifique una fusión. Superficie
`dataforge.tool-surface/0.4.0`, 26 herramientas.

Queda su simétrico, que aún no está: 678 hashes genuinamente compartidos entre
asuntos, que **no** se pueden consolidar.

#### El reencuadre de la clasificación: perfil, no veredicto por archivo

**Bloqueado: necesita su propia ADR antes de escribirse.**

El trabajo original produjo, por archivo, una categoría y un motivo —
`excluido_no_juridico` 19.413, `asesoria_main` 12.426, `correos` 7.873,
`revision_origen_mixto` 5.576, `periciales` 2.331, sobre 47.982 decisiones.
DataForge no tiene ese verbo: sabe decidir sobre *elementos de revisión*
estructurales, pero no puede decir «este archivo va a esta raíz por este
motivo».

Lo que la evidencia desaconseja es que el modelo juzgue 158.219 archivos. El
trabajo original **no acabó así**: acabó con marcadores y raíces declaradas. La
forma que reproduce ese resultado es un trío:

- **`propose_profile(evidence)`** — el agente propone **un perfil**: raíces,
  marcadores, reglas. Pequeño, auditable, reutilizable.
- **`validate_profile(profile)`** — el motor lo valida *fail-closed*, sin
  escribir nada.
- **`apply_profile(profile)`** — determinista y reproducible sobre todo el
  corpus, sellando digest y versión.

Veinte reglas que un humano lee, frente a 158.219 juicios que nadie puede
auditar. Encaja con ADR-0026 (perfiles declarativos) y con
`destination_roots` de M2.2, que ya declara las raíces en el perfil en vez de
en un `match`.

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

**Estado: primera pieza puesta (2026-08-01).** El crate existe con las **cuatro
fronteras duras** de ADR-0041 §4 y el motor de veredicto.

La decisión de diseño que lo vertebra es **qué va en código y qué va en datos**.
Las fronteras duras son invariantes, no preferencias, y viven en Rust: una
frontera que el llamante puede editar no es una frontera. Los pesos —qué copia
gana, cuánto penaliza un contenedor genérico, qué margen exige una
auto-aprobación— dependen del corpus y viven en `RuleParams`, versionados y con
digest. `HARD_BOUNDARY_COUNT` entra en `frozen_contracts` justamente para que
mover un invariante a la mitad afinable rompa el build y no el archivo.

Un test —`no_parameter_can_authorise_past_a_hard_boundary`— fija que ningún
parámetro, por permisivo que sea, autoriza por encima de una frontera.

De las cuatro fronteras solo una **deniega**: consolidar dos documentos
distintos no es un juicio, es una contradicción. Las otras tres describen
situaciones que un humano sí puede resolver legítimamente —reutilización entre
asuntos, un destino que se puede vaciar, un dominio protegido revisable archivo
a archivo— así que van a `revisar/` y el run continúa, que es la garantía de
no-bloqueo de RFC-0002.

El digest se **reverifica en cada llamada**, no una vez al cargar: un conjunto
que derivó entre leerse y usarse es exactamente el caso que una comprobación al
cargar no ve (amenaza A4).

**Pendiente del hito:** la tabla `rule_sets` persistida (migración 0021) y
conectar los pesos afinables al `location_cost` que hoy usa constantes en
`df-db`. La clasificación de estado de injerto (ADR-0041 §3) espera a que se
resuelva la precedencia de ADR-0045: sin eso, sus veredictos serían correctos e
inalcanzables, igual que le pasa hoy a `ContainedTreeReplica`.

### M2.5 — Consentimiento por política

[ADR-0042], extensión de ADR-0034. El humano aprueba **una vez** una política
de divulgación con presupuesto de llamadas, tokens y gasto. Agotado el
presupuesto, lo ambiguo restante va a `revisar/`: degrada, no bloquea, no
dispara la factura. La clave sigue en el almacén de credenciales del sistema.

**Estado: decisión implementada (2026-08-01)**, en `df-ai::policy`. ADR-0034
pide aprobación humana **por petición**, que es correcto cuando hay alguien
delante e inviable en un run autónomo: una cola de 5.334 elementos no se aprueba
de prompt en prompt, y un run que se para a preguntar es un run que no termina.

Tres propiedades, cada una con test:

- **Se audita antes de tocar la clave o la red.** `authorize` es una decisión
  pura sobre el manifiesto y el consumo acumulado, y devuelve antes de enviar
  nada. Auditar después registraría una divulgación ya ocurrida — eso es un
  log, no un control.
- **Agotado degrada, no rechaza.** `Exhausted` y `Refused` son cosas distintas
  a propósito: el primero significa «manda el resto a `revisar/` y sigue», el
  segundo «esto nunca estuvo permitido».
- **Un campo fuera de la política se rechaza, no se recorta.** Recortarlo
  enviaría *algo* que el humano no aprobó mientras el digest seguiría diciendo
  que describe lo pactado.

Dos detalles que costaría caro equivocar: el presupuesto se comprueba contra el
total que la invocación **produciría**, no contra el ya gastado, así que la
llamada que cruzaría la línea no la cruza; y `0` significa «no permitido», nunca
«ilimitado» — un presupuesto que nadie fijó no puede ser un presupuesto sin fin.

**Pendiente del hito:** persistir la política y su auditoría de consumo
(migración 0022) y conectar `authorize` a la ruta de transporte de `df-ai`,
que hoy sigue usando el token por petición de ADR-0034.

### M2.6 — `df-agent`

**Estado: lógica de decisión implementada (2026-08-01)**, en `crates/df-agent`.
Las fases, los presupuestos y el cortacircuitos, sin E/S, con 11 tests.

Lo que fija el crate es **la garantía de no-bloqueo**. `assess` no puede
devolver «para y espera»: su peor respuesta es `DegradeToReview`, que significa
*manda lo que queda a `revisar/` y pasa a la fase siguiente*. Duda, ambigüedad,
presupuesto agotado y cortacircuitos disparado resuelven todos igual, porque un
run que se detiene a esperar a un humano en un disco lento sigue sin terminar
dos días después — que es justo el fallo que RFC-0002 vino a quitar. Hay un
test, `the_loop_can_never_block`, que lo prueba contra entradas extremas y que
es donde habría que defender cualquier variante futura que espere.

El orden de fases es un tipo, no una convención: `writes_to_destination()` es
falso hasta `Execute`, así que «pensar antes de copiar» se comprueba en vez de
documentarse. Un dry-run llega hasta `Freeze` incluido — uno que no congelara
estaría previsualizando un plan que aún puede cambiar, que no es lo que se
quiere ver.

El cortacircuitos tiene **suelo de muestra**: sin él, el primer elemento ambiguo
de un run es una tasa del 100% y salta con evidencia de uno. Y la comparación es
estrictamente mayor, no mayor-o-igual: un umbral de 0,35 significa «hasta un 35%
es aceptable», que es como lo lee cualquiera que lo configure.

**Pendiente del hito:** conducir de verdad el motor por `df-tools`, el pre-vuelo
de espacio, los buckets `revisar/_ilegible/` y `revisar/_verificacion-fallida/`,
la reanudación desde el manifiesto y el informe origen→destino exportable.

#### Vitalidad del run (2026-08-10) — precondición, no robustez opcional

Reanudar exacto desde el manifiesto exige antes saber **si hay alguien
copiando**. Hoy no hay forma: ni PID, ni host, ni fase, ni latido en todo
`crates/`. Un agente que se reconecta no puede distinguir un run vivo de uno
muerto hace tres días, y el proyecto de la prueba de la 1.0 —clavado en
`EXECUTING`— es la demostración. Va con el desacoplamiento `job_start`/
`job_status` de M2.1, porque son la misma costura vista desde los dos lados.

#### `export_delivery_package` — el criterio de aceptación real

El trabajo original no se aceptó por una métrica técnica. Se aceptó, literal,
*«que el asesor no tenga desconfianza porque se haya podido perder material
alguno»*. Por eso el entregable acabó siendo informe + CSV de trazabilidad +
manifiesto SHA-256 dentro de la propia carpeta de salida.

El motor ya tiene todo el dato —manifiesto congelado, procedencia por
operación, ledger encadenado—; lo que no tiene es **la forma de entregarlo**.
Un resultado correcto que no se puede demostrar no sirve, que es la diferencia
entre terminar la tarea y que te la den por buena.

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
| Paquete de entrega | Informe + trazabilidad + manifiesto SHA-256 en la salida |

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
  **Reducido en parte el 2026-08-10:** para árboles injertados el umbral ya no
  se supone, se midió —99,1 % automático / 0,9 % a revisión sobre 135.378
  archivos— porque el trabajo original dejó las 47.982 decisiones etiquetadas y
  se pueden contrastar. Sigue abierto para todo lo demás: una cifra medida en
  un archivo jurídico no es un umbral universal, y el modo sombra sigue siendo
  la forma de saberlo.
  **Y agravado el mismo día:** al abrir esa base resulta que de las 47.982
  decisiones **ninguna la revisó un humano** — las tomó todas una fórmula de
  puntuación y nadie las ha comprobado nunca. No es que falten datos de
  calibración: es que los que hay son de una heurística sin auditar. Medición y
  muestra en
  [tres-decisiones-con-evidencia.md](tres-decisiones-con-evidencia.md).
- **Suponer que la política de duplicados basta.** Se dio por hecho, en esta
  misma línea de trabajo, que elegir `CONSOLIDATE_ALL` produciría una salida
  deduplicada de unos 204 GB. Medido, el ahorro alcanzable es de 5,45 GB. La
  cifra falsa se sostuvo varias conversaciones porque nadie había ejecutado la
  política contra datos reales. De ahí que M2.3 sea precondición y no adorno,
  y de ahí que la definición de hecho lleve umbrales verificables.
