# Auditoría: rutas que producen un resultado equivocado sin que nada se queje

**Fecha:** 2026-08-17
**Método:** lectura de código, sin ejecutar el pipeline. Cada hallazgo cita
fichero y línea, y cada uno pasó por un intento explícito de refutarlo antes de
entrar aquí.

## La clase de defecto

Un run de campo de 444 GB sobre un archivo jurídico corrió quince horas bajo el
perfil `generic`, cuyo `protected_markers` está vacío. Dedujo duplicados **a
través** de carpetas `expediente`, que es exactamente lo que la regla 9 de
RFC-0001 existe para impedir: cada expediente debe sostenerse solo como unidad
probatoria.

El perfil correcto existía desde antes. `profiles/legal/profile.json` declara
`expediente`, `exp`, `pericial`; la cadena está montada hasta
`protected_boundaries`; y `crates/df-planner/tests/m02_legal_profile.rs` prueba
de punta a punta que esas fronteras sobreviven a una consolidación agresiva.
Todo verde. Lo que faltó fue escribir `--profile legal`, porque el flag tiene
`default_value = "generic"`.

No falló nada. Ninguna etapa avisó. La verificación pasó limpia. El resultado
era estructuralmente equivocado.

**Esa es la clase:** no un fallo, no un panic, no una verificación en rojo, sino
un run que termina en verde y entrega algo que viola el criterio que el usuario
necesita. Son las que impiden operar sin humano delante, porque un agente
conduciendo DataForge no recibe ninguna señal de que algo fue mal. Un error que
se anuncia no pertenece a esta lista.

### Cuánto costó, medido

Recorriendo el archivo real y clasificando cada nombre de carpeta con el
`Profile::classify()` de verdad —no una reimplementación—, sobre 36.458 carpetas
y 158.219 archivos:

| | `generic` (el que corrió) | `legal` (el que existía) |
| --- | ---: | ---: |
| Carpetas protegidas | **0** | **255** |
| Archivos bajo frontera protegida | **0** (0,0 %) | **16.955** (10,7 %) |

Reparto de los marcadores que casan: `pericial` 98, `asunto` 86,
`correspondencia` 33, `expediente` 16, `procedimiento` 12, `clientes` 4,
`expedientes` 4.

El `0` de la izquierda reproduce exactamente el `Protected bounds: 0` que
reportó el run, lo que confirma por medición —y no por deducción— qué perfil se
usó. **16.955 archivos, el 10,7 % del archivo, quedaron expuestos a
consolidación entre contextos que la regla 9 prohíbe.**

> Nota lateral, sin resolver: ese recorrido cuenta **158.219** archivos, que es
> exactamente la cifra citada en ROADMAP-2.0 y no los 155.906 que midió el
> escaneo de DataForge y que el documento de campo registra como «diferencia no
> explicada» de 2.313 archivos. Un recorrido crudo de directorios y el
> inventario del motor no coinciden. No es un hallazgo todavía —el recorrido
> cuenta toda entrada que no sea directorio, incluidos enlaces y puntos de
> reanálisis, y el escaneo puede excluirlos a propósito— pero acota la
> diferencia a algo comprobable en vez de dejarla como misterio.

---

## 1. Un origen vacío entrega la garantía de que se leyó todo

**Gravedad: bloquea la autonomía.**
`crates/df-scan/src/lib.rs:195-215`, `crates/df-facade/src/lib.rs:2765`

`scan_project` elige su estado terminal mirando solo si hubo cancelación:

```rust
let (run_status, next_state) = if cancelled {
    (ScanRunStatus::Cancelled, ProjectState::ScanPaused)
} else {
    (ScanRunStatus::Completed, ProjectState::Scanned)
};
```

`counters.files` no participa. Y aguas abajo no hay suelo en ninguna parte:
`hash` encola cero trabajos y llega a `HASHED`; `analyze_project` sella un
resumen de ceros y llega a `ANALYZED`; `create_plan` emite el único
`CREATE_DIRECTORY` de la raíz y llega a `PLAN_READY` con `blocked = 0`;
`occurrence_coverage_problems` se satisface trivialmente sobre un conjunto
vacío, así que `plan validate` y `approve_plan` pasan; `execute` completa un
`mkdir`; `verify_project` revisa ese único artefacto y devuelve `COMPLETED`.

Entonces `export_delivery_package` escribe, literalmente:

> *Everything in the origin was read and identified by hash.*

**Escenario.** El operador apunta `--source` a `D:\Archivo` en una máquina donde
esa letra resuelve hoy a un stub vacío: volumen sin montar, recurso de red
reasignado, punto de montaje cambiado. Es el fallo ordinario del hardware sobre
el que vive un archivo de 444 GB. Quince segundos después todas las etapas están
en verde, `verify` dice `COMPLETED`, y `delivery.md` le entrega al destinatario
una declaración de garantías afirmando que el origen se leyó entero. El destino
contiene un directorio vacío.

**Qué debería haberlo cogido.** `validate_project`
(`crates/df-scan/src/lib.rs:79-83`) es el sitio exacto y ya rechaza la entrada
degenerada vecina —cero raíces de origen— con «the project has no source
roots». Nunca pregunta si una raíz que sí existe contiene algo. La verificación
no ayuda por construcción: verifica el plan contra la salida, y un plan de cero
copias lo satisface una salida de cero copias.

**Intento de refutación.** Busqué un suelo en cinco sitios y no hay ninguno: no
existe guarda `files == 0` en `df-scan`, `df-planner`, `df-executor` ni
`df-facade`; la única guarda de vacío del paquete de entrega es
`if entries.is_empty()`, que no dispara porque el `CREATE_DIRECTORY` de la raíz
mete una entrada en el manifiesto; e `integrity::check` comprueba pragmas,
migraciones y la cadena del ledger, ninguna de las cuales es una afirmación
sobre contenido. Los recuentos **sí** se imprimen (`Files: 0`), y esa es toda la
mitigación: nada distingue «el origen no contiene nada» de «nos apuntaron a
nada».

## 2. La búsqueda de contenido trunca sin total y sin marca de truncado

**Gravedad: degrada el criterio.**
`crates/df-search/src/lib.rs:392-396`, `crates/df-facade/src/lib.rs:413-418`,
`apps/cli/src/main.rs:363`

`search_index` devuelve `Vec<SearchHit>` desde
`TopDocs::with_limit(request.limit)`. Tantivy sabe cuántos documentos casaron;
ese número se descarta. `ContentSearchOutcome` lleva `run_id`, `index`, `query`,
`hits` — sin total y sin `has_more`. La CLI imprime `Hits: {}` bajo un
`default_value_t = 20`, y la superficie de agente usa el mismo 20 por defecto.
`content_search` está deliberadamente **fuera** de `report_collections`, así que
nunca recibe una `Page`.

**Escenario.** Se pregunta si el archivo menciona a una contraparte. Hay 3.400
coincidencias; llegan 20, todas del mismo expediente porque puntúan más alto, y
la respuesta es que el asunto aparece en un sitio. Con `MAX_RESULTS = 100` y
`MAX_OFFSET = 10_000`, ni siquiera un llamante que sospeche del truncado puede
llegar al total.

**Qué debería haberlo cogido.** El propio repositorio enuncia la regla y la
aplica dos veces en otros sitios. `crates/df-tools/src/lib.rs:526`: *«a
truncation the caller cannot detect is not pagination — it is a wrong answer
that looks like a right one»*. Y `crates/df-query/src/lib.rs:467` pide
`max_rows + 1` justamente para **fallar en alto** en vez de truncar. La búsqueda
es la única superficie de evidencia exenta de los dos mecanismos.

## 3. El recuento de exclusiones se calcula, se tira, y falta justo cuando importa

**Gravedad: degrada el criterio.**
`crates/df-hash/src/lib.rs:182`, `crates/df-db/src/inventory.rs:1103-1110`

`enqueue_hash_jobs` devuelve `EnqueueOutcome { enqueued, excluded }` y el hasher
descarta el valor. `HashOutcome` no tiene campo `excluded`, `InventorySummary`
tampoco, el payload de `HASH_COMPLETED` tampoco, y `print_hash` no lo imprime.
El único sitio donde el número llega al ledger es dentro de `HASH_STARTED`,
detrás de `if enqueued > 0`.

O sea: **el caso en que el número más importa —una regla de exclusión más ancha
de lo previsto, que casa con todo, `enqueued == 0`— es exactamente el caso en
que no se registra.** `verify_chain` comprueba secuencia y hashes, nunca
gramática de eventos, así que un `HASH_COMPLETED` sin `HASH_STARTED` y sin
recuento pasa todas las auditorías que el motor ofrece.

**Mitigación honesta, y por eso va tercero y no primero.**
`hash_exclusion_report` existe y está cableado en la superficie de agente, pero
**no** en la CLI: `ReportCommand` no tiene subcomando `exclusions`, así que una
persona en un terminal no puede preguntarlo. Dos etapas más tarde el problema sí
aflora, mal etiquetado: cada ocurrencia excluida cae en el último brazo del
planner como `Blocked` con «no verified content identity». El resultado
equivocado se ve, pero después de que el operador ya haya aceptado una etapa de
hash en verde.

## 4. `CONSOLIDATE_WITHIN_CONTEXT` solo descarta archivos cuya única diferencia es el nombre

**Gravedad: degrada el criterio.**
`crates/df-planner/src/lib.rs:1062-1069`, `crates/df-domain/src/duplicate_policy.rs:228`

`classify_duplicate_set` devuelve `WithinSameContext` solo cuando todos los
miembros comparten raíz de origen **y carpeta padre**. Dos archivos idénticos
byte a byte en el mismo directorio tienen necesariamente **nombres distintos**.
Así que ese tipo —el único que `ConsolidateWithinContext` consolida— es por
construcción el conjunto de duplicados que no se diferencian en nada salvo el
nombre. La justificación de la propia política, «where keeping N identical files
adds nothing», es falsa precisamente para ese conjunto.

**Escenario.** Un expediente guarda `escrito.pdf` y `escrito - FIRMADO.pdf`,
idénticos byte a byte porque la copia firmada se produjo renombrando. Con esa
política el plan emite `SKIP_REPRESENTED` para el nombre más largo, la salida
conserva solo `escrito.pdf`, y del archivo entregado desaparece el hecho de que
existía una versión firmada. `plan create` sale 0, `plan validate` dice OK, y
`verify` pasa porque solo re-comprueba lo que el plan prometió.

Hay divulgación parcial y conviene decirlo: `skipped_represented` se cuenta, la
operación lleva su razón, la fila aparece en `traceability.csv` con destino
vacío y `delivery.md` imprime «Without a recorded destination: N». Lo que falta
es cualquier afirmación de que eran archivos cuya única diferencia con uno
conservado era el nombre — y ese número va mezclado con `NO_ACTION` y `BLOCKED`
(ver el casi-hallazgo de abajo), así que se lee como ruido.

---

## Dos cosas que se investigaron y **no** son un problema

Vale tanto como los hallazgos, porque dice qué está ya cubierto.

**El reuso incremental está bien clavado.** La sospecha era que ADR-0035
reutilizara trabajo por contenido inalterado mientras la *configuración*
cambiaba, devolviendo la respuesta del run anterior a una pregunta distinta. No
ocurre. `--profile` solo existe en `project create`
(`apps/cli/src/main.rs:426`), la columna se escribe una vez
(`crates/df-db/src/repository.rs:121-137`), hay un fichero SQLite por proyecto y
`require_matching_completion_scope` (`crates/df-db/src/analysis.rs:296-325`)
compara `profile_id` y `profile_sha256` y devuelve `Conflict` si no cuadran.
Extracción, similitud y media llevan un `config_digest` dentro de su clave de
reuso, y `ensure_run_matches_spec` compara los catorce parámetros campo a campo
en vez de fiarse del digest. Tampoco hay reuso entre proyectos: el SQL lo acota
con `project_id`, y además no hay otro proyecto en el fichero del que reutilizar.

**Un buen número de sospechas mueren contra una guarda existente.** Contexto de
carpeta que no hereja hacia abajo: refutado, `duplicate_members` recorre
`ancestor_paths` y se queda con el tipo más restrictivo. `CONTAINED_TREE_REPLICA`
tirando la última copia: refutado por tres caminos distintos. Una exclusión sin
criterios que se coma el archivo: refutado tres veces, incluida una que impide
cargar el perfil. Un worker de PDF ausente produciendo un índice vacío en
silencio: refutado, marca `limited` y fuerza `PARTIAL`. `content_query`
truncando: refutado, pide una fila de más y falla. Y varios `let _ =` y
`continue` del executor y el verificador: todos escriben un `PROBLEM` o un
contador que fuerza salida 3 antes de continuar.

## Un casi-hallazgo: la advertencia que enseña a ignorar advertencias

`destination_tree` cuenta como `without_destination` **toda** operación no
directorio con destino nulo (`crates/df-db/src/plans.rs:1583-1588`), lo que
incluye `SKIP_REPRESENTED`, `NO_ACTION` y `BLOCKED`. Un plan consolidante
correcto hace por tanto que `plan tree` imprima «WARNING: N copy operation(s)
have no destination recorded» y salga 3.

Falla del lado seguro, así que no es de la clase. Pero entrena al operador —y a
cualquier agente que mire el código de salida— a ignorar la única advertencia
que señalaría una copia real sin sitio donde aterrizar. Y es el mismo número que
el paquete de entrega presenta como «Without a recorded destination».

---

## El criterio no vive donde la documentación dice

Aparte de la auditoría, y relacionado con el mismo incidente del perfil:

- **`df_rules::evaluate` no tiene ningún llamante fuera de sus propios tests.**
  El motor de reglas está inerte. El propio código lo dice
  (`crates/df-facade/src/lib.rs:2948`): *«Storing is not gating: nothing
  consults these at decision time yet.»*
- **`RepresentativeWeights` no se consume.** El ranking real que corre son dos
  constantes en `crates/df-db/src/dedup.rs:38-40`: `LOCATION_WEIGHT = 100` y
  `COPY_MARKER_COST = 10`, más un orden lexicográfico por
  `(cost, modified, depth, path_length, absolute_path)`.

ADR-0041 se propuso justamente para no «fijar en código pesos que el usuario
debería poder afinar por corpus o dominio», y hoy están fijados en código.

**Un matiz que hay que dejar claro, porque me equivoqué al leerlo la primera
vez.** La fórmula ponderada de ADR-0041 §2
(`−8·profundidad − 1,1·longitud_ruta + 15·mtime_más_antiguo`) **es** la que
generó la verdad de referencia del banco: `decisions.auto_reason` en
`DataForge_Audit_Work\dataforge.sqlite` dice literalmente
`depth -8; path_length -1.1; oldest_mtime +15` en las 47.982 filas, y
`manual_file_id` está vacío en todas: el humano nunca corrigió a mano una
elección de representante. Así que conectar esos pesos no «desharía» el 93,3 %
—lo llevaría al 100 % contra el script—. Lo que el orden lexicográfico añade es
la penalización de contexto de §18.3, que el script no tenía: una mejora
deliberada *sobre* la referencia, no concordancia con ella. Los dos números que
sí están medidos y registrados están en `dedup.rs:278`: profundidad sola
concuerda el 72,7 % y este orden el 93,3 %.

## La lección de método, que es la misma cuatro veces

El diagnóstico del perfil salió de leer el síntoma en vez del directorio.
`Protected bounds: 0` admitía dos explicaciones —no hay perfil, o no se eligió—
y se escogió una y se escribió como hecho; `ls profiles/` la habría descartado.
Los tres separadores hardcodeados de la semana pasada se «arreglaron» dos veces
razonando sobre la causa en lugar de leer el log que ya estaba disponible. Y el
matiz del párrafo anterior se afirmó al revés antes de abrir la base de datos
que lo contenía.

Razonar sobre la causa probable es más rápido que comprobarla. Por eso se elige,
y las cuatro veces salió más caro.
