# RFC-0002 — Reconstrucción agéntica: de IA asistida a agente autónomo por lotes

**Estado:** Borrador
**Fecha:** 2026-07-21
**Autor:** (pendiente)
**Reemplaza / modifica:** complementa RFC-0001 §23 (IA asistida), §25 (reglas
declarativas) y §45 (roadmap); no modifica ninguna garantía absoluta.

## Resumen

DataForge 1.0 tiene un motor de reconstrucción determinista y una IA
**asistida** (`df-ai`, ADR-0034) que se detiene deliberadamente en
explicaciones y sugerencias: no muta el sistema de ficheros, no planifica, no
aprueba y no ejecuta. Este RFC define cómo convertir eso en un **agente que usa
la herramienta de extremo a extremo**: el usuario describe en lenguaje natural
qué quiere de un directorio caótico, y el agente —con el proveedor de IA que
sea— entiende los problemas del directorio, propone un plan completo, lo aprueba
bajo reglas y lo ejecuta **sin detenerse nunca**. Cuando algo es ambiguo o no
sabe manejarlo, no bloquea esperando a un humano: lo copia a una carpeta
`revisar/` (espejo del árbol de salida) con su motivo y **continúa hasta
terminar**. El humano revisa `revisar/` y el informe **después**. Es autonomía
**por lotes, no bloqueante y autocompletable**, pensada para discos viejos y
lentos donde un trabajo dura horas o días.

## Motivación

DataForge nace de un caso real: ordenar un archivo documental de más de diez
años, más de 200 GB dominados por duplicados, un caos. La solución original fue
conversacional (una IA, lenguaje natural, iterando hasta el output deseado).
Este RFC "vitaminiza" esa experiencia en producto: mantener el lenguaje natural
y la comprensión de la IA, pero **agéntico y desatendido**.

El pipeline 1.0 exige un humano en cada decisión (aprobar el plan, resolver cada
item de la cola de revisión). En un disco lento con cientos de miles de archivos
eso no escala y, peor, **para el trabajo**: un run que tarda dos días no puede
quedarse bloqueado esperando a que alguien responda una duda.

La oportunidad singular de DataForge es que **la autonomía aquí no es un
problema de seguridad, sino de política**. Los invariantes del motor acotan el
radio de daño por diseño: origen inmutable, sin borrado ni sobrescritura, toda
salida es copia verificada, SQLite es la única fuente de verdad, el manifiesto
se congela al aprobar. Lo peor que puede producir un agente totalmente autónomo
es una **reconstrucción subóptima** —una copia mal organizada—, nunca una
pérdida de datos, y siempre es **reversible** (se borra el árbol de salida; los
orígenes quedan intactos) y **auditable**.

## Diseño propuesto

### Dos principios rectores

1. **La IA nunca es la autoridad.** La autoridad es una **regla declarativa**
   que la IA propone satisfacer y que el motor **verifica de forma
   determinista**. El modelo aporta clasificación y propuesta; la decisión de
   aprobar la toma `df-rules` sobre evidencia recalculada localmente. La
   autoconfianza que reporte el modelo nunca se cree.

2. **El vocabulario de acciones está acotado y es no destructivo.** La libertad
   de la IA está en **clasificar y decidir la estructura destino**. Las
   **acciones** son solo: *copiar* el original a una ubicación de la estructura
   de salida (incluida `revisar/`) y *anotar*. Nunca borra, nunca mueve ni
   destruye el original, nunca sobrescribe. Esto es lo que convierte "autónomo y
   entiende cualquier problema" en seguro: el fallback universal para "no sé qué
   hacer con esto" es `revisar/`. La IA entiende "lo que sea" en el sentido de
   **saber cuándo no sabe** y poner eso en cuarentena, no en el de inventarse
   acciones fuera del vocabulario.

### La escalera de autonomía

| Nivel | Autoridad del gate `approve` | Rol del humano | Estado |
| --- | --- | --- | --- |
| **L0** | Humano | Hace todo; la IA solo explica | ✅ Implementado (df-ai, M0.7) |
| **L1 — Copiloto** | Humano | Aprueba el plan que la IA propone | Punto de parada seguro (misma base) |
| **L2 — Autónomo por lotes** | **Regla declarativa** (`df-rules`) | (Opcional) confirma la intención **al inicio**; revisa `revisar/` **al final** | **Objetivo de este RFC** |

L1 y L2 comparten toda la infraestructura de base (superficie de herramientas,
esquema de plan, reglas, auditoría). La única diferencia es **quién ocupa el
gate**: en L1 el humano; en L2 el motor de reglas. Por eso implementar L2 pasa
obligatoriamente por construir esa base común, que es un L1 seguro y entregable
por sí mismo.

### El ciclo de vida de un trabajo (dos fases de lenguaje natural)

**Fase A — Captura de intención (inicio, conversacional, rápida).** El usuario
describe el directorio y qué quiere ("es un caos de diez años; ordénalo; fotos
por año; sepárame lo jurídico; lo dudoso a revisar"). La IA traduce eso a una
**política + reglas para este trabajo**. Opcionalmente, el agente devuelve *qué
entendió* para una confirmación humana única antes del run largo (evita dos días
de trabajo sobre un malentendido). Configurable: con reglas por defecto de
confianza, el trabajo corre 100 % desatendido sin esta confirmación.

**Fase B — Ejecución autónoma (desatendida, larga).** El agente inventaría,
analiza y **produce el plan completo**, lo **congela** y lo ejecuta sin
detenerse. El grueso de la clasificación lo resuelven **heurísticas
deterministas que el motor ya tiene** (dedup por hash, firmas de carpeta,
marcadores de contexto); el modelo de IA se reserva para la **cola ambigua**, y
lo que sigue siendo dudoso va a `revisar/`. No se necesita una llamada de IA por
archivo: el LLM pesa en la Fase A y va ligero en la Fase B.

### Diagnóstico y modos de reconstrucción (recuperar vs. inventar)

Antes de decidir colocaciones, el agente **diagnostica qué hay**: qué fracción
del contenido encaja en tree-clones coherentes con un árbol canónico latente
frente a qué queda suelto. De ese diagnóstico salen dos modos, casi siempre
mezclados:

- **Modo 1 — Recuperación.** Existe un orden claro enterrado bajo réplicas
  anidadas (el caso original: el mismo subárbol arrastrado durante años a
  `Backup/`, `Escritorio/…`, etc.). El agente **desentierra el árbol canónico** y
  descarta las réplicas. No inventa nada.
- **Modo 2 — Invención.** No hay árbol dominante recuperable (contenido disperso,
  o el que hay es pobre). El agente **diseña una taxonomía lógica** (por fecha,
  tipo, tema/entidad) —aquí el LLM aporta de verdad— y **la explica en el
  informe**.

**Recuperación canónica (Modo 1) — validada contra el trabajo original.** El
motor lo hace en **dos pasadas**; la primera resuelve la mayoría **sin IA**.

**Pasada 1 — Deduplicación por contenido (elige el representante).** Se agrupan
los archivos por **SHA-256** (grupos de duplicados exactos) y, dentro de cada
grupo, se elige un **representante** con la fórmula real del trabajo original:

> `score = −8·profundidad − 1,1·longitud_ruta + 15·(mtime más antiguo)`

Gana la instancia **más superficial, de ruta más corta y con el mtime más
antiguo**. La clave del mtime: en duplicados **byte-idénticos** el original
conserva su fecha vieja y las **copias** suelen recibir una fecha más nueva al
copiarse — por eso **el más antiguo es el original**. (Matiz: esto es para
duplicados **exactos**; entre **variantes** de distinto hash del mismo documento,
la **más reciente** es la versión viva.) La salida es el repositorio depurado,
poblado con la ruta del representante. Esta pasada **ya limpia la mayoría de los
injertos**, porque el score prefiere rutas limpias.

**Pasada 2 — Auditoría estructural de árboles injertados.** Detecta la
*contaminación de contexto*: subárboles raíz **injertados** dentro de expedientes
por arrastres/copias (el mismo subárbol repetido a 1, 2, 3… niveles de
anidación). Cada grupo injertado se clasifica por estado:

- `mirror_exact_by_path` / `mirror_or_duplicate_near_complete` → réplica (casi)
  completa: **colapsable con seguridad**.
- `mixed_duplicate_and_unique_review` → mezcla de duplicado y contenido único →
  **a revisión** (es el clon **parcial**, deuda de ADR-0023).

De ahí salen dos destinos, y esta distinción es el **núcleo de `df-rules`**:

- **Reemplazo limpio** — existe una ruta **más limpia con el mismo SHA-256 exacto**
  → sustitución **auto-aplicable** (contenido idéntico probado, riesgo nulo).
- **Contenido único en contexto sospechoso** — un archivo **sin duplicado** que
  vive en un árbol injertado → no se puede colapsar y su ubicación es incierta →
  **`revisar/`**.

**Reglas duras heredadas del caso real:**

- **Nunca deduplicar por nombre:** mismo nombre + hash distinto = **documentos
  distintos** (variantes). Solo el hash decide identidad.
- **Evidencia compartida entre asuntos** (misma imagen/hash en varios
  expedientes) → siempre a `revisar/`: puede ser reutilización legítima o
  contaminación de expediente; no lo decide la máquina.
- **Origen suelto** (una imagen de pericial que también aparece en
  `Fotos/Descargas/Escritorio`) → probable origen previo de cámara/temporal → se
  marca, no se colapsa a ciegas.

**Contrato de salida del Modo 1: `ruta legit + contenido sin duplicar`.**

**Conservador vs. agresivo lo elige el usuario, no una constante.** Cuando existe
un árbol canónico pero es mejorable, el agente **pregunta en la Fase A**:
*conservador* (recupera tu estructura tal cual; las mejoras las marca en el
informe como sugerencia, no las impone) o *agresivo* (rehace al orden que juzga
mejor, documentándolo). Ambos soportados; es una opción del trabajo, resuelta
antes del run largo, no durante.

### Piensa primero, copia después

Toda la parte "cara de pensar" (clasificación, IA, resolución de duplicados)
ocurre **antes** de mover un solo byte y produce el **plan completo**: para cada
archivo, su destino en la estructura de salida, o su ubicación en `revisar/` con
el motivo. Ese plan se **congela** (manifiesto de ejecución, migración 0004,
triggers de inmutabilidad) y la copia lenta se limita a **ejecutar un contrato
fijo y reanudable** — ya no decide nada. En un disco lento esto es decisivo: la
fase rápida (metadatos) decide todo; la fase lenta (E/S) es tonta, resumible y
muestreable **antes** de empezar.

### La carpeta `revisar/`: espejo del árbol de salida

`revisar/` **reproduce la misma jerarquía de carpetas que el output** y coloca
cada item dudoso **en su mejor ubicación estimada** dentro de ese espejo. Así,
al revisar, no ves un motivo abstracto: ves *dónde iría el documento y junto a
qué otros*, que es justo el contexto que necesitas para decidir de un vistazo. Y
la revisión es la más barata posible: **aceptar = mover de `revisar/<ruta>` a
`output/<ruta>`** (la ruta relativa ya está calculada).

- **Eje principal:** espejo del output, item en su mejor apuesta.
- **Hueco neutro:** si el agente no tiene ni apuesta de ubicación, va a
  `revisar/_sin-ubicar/`. El espejo cubre "tengo apuesta pero dudo"; el hueco
  neutro cubre "no sé ni por dónde". Entre ambos, **nada se pierde ni se salta
  en silencio**.
- **Buckets técnicos reservados** para fallos, no clasificación:
  `revisar/_ilegible/` (error de lectura del origen), `revisar/_verificacion-
  fallida/` (la copia no re-hashea igual), `revisar/_colision/` (si la política
  de colisión así lo decide).
- **El motivo es metadato, no carpeta.** Viaja como anotación junto al item y se
  **agrega en el informe final** ("47 en revisar: 30 duplicado ambiguo, 12
  contexto incierto, 5 protegido jurídico"). Conserva el *por qué* sin romper el
  espejo posicional.

`revisar/` no es nada especial para el motor: es **otro conjunto de destinos de
copia**, sujeto a las mismas garantías (copia-solo, sin sobrescribir, origen
intacto, ruta preservada bit a bit).

### Robustez para discos viejos y lentos

Estas son reglas duras, no "bonito tener", porque son el caso de uso real:

- **Errores de lectura del origen** (sectores malos): el archivo va a
  `revisar/_ilegible/` con el error registrado; el run **no aborta jamás**.
- **Colisiones de nombre en destino** (masivas en archivos caóticos): como nunca
  se sobrescribe, se desambigua de forma **determinista** (sufijo por hash de
  contenido) o el item va a `revisar/_colision/`, según política.
- **Pre-vuelo de espacio:** antes de arrancar la copia, se compara el tamaño
  planificado contra el libre del destino; si no cabe, se informa y **no se
  empieza** (nunca llenar el disco a mitad de un trabajo largo).
- **Reanudación exacta:** un run interrumpido retoma desde el manifiesto
  congelado + el ledger de operaciones completadas; nunca reinicia ni duplica.
  Lo ya colocado correctamente (identidad física, fingerprint v2) es un no-op.
- **Verificación tras copia:** el verificador re-lee y re-hashea; si un archivo
  no verifica → `revisar/_verificacion-fallida/` y el run continúa. Nunca acepta
  una copia mala en silencio.
- **Origen cambiado durante el run:** el fingerprint v2 (identidad física +
  change time) lo detecta; ese item va a `revisar/`.

### Componentes nuevos

1. **Superficie de herramientas sobre `df-facade`** (ADR-0043 propuesta).
   Operaciones tipadas invocables por un agente: `scan`, `analyze`, `query`,
   `plan_propose`, `approve`, `execute`, `verify`, `report`. Dos encarnaciones,
   mismo contrato: API in-process (Rust) y **servidor MCP** para que cualquier
   runtime de agente (incluido Claude) maneje DataForge **con el proveedor que
   sea**. El agente llama las **mismas operaciones que un humano**; no hay puerta
   trasera al congelado del manifiesto ni al verificador.

2. **`df-rules`** (ADR-0041 propuesta; RFC-0001 §25). Evalúa un plan contra
   reglas y devuelve `Autorizar | A-revisar | Denegar` **con el id de la regla
   que lo determinó**. Fronteras duras *fail-closed* (nunca fusionar proyectos;
   nunca tocar orígenes jurídicos/protegidos; destino vacío obligatorio; no
   superar presupuesto de divulgación ni de espacio) y reglas de política (qué
   auto-aprobar según confianza recalculada). Reglas **versionadas y con
   checksum**, misma disciplina que las migraciones: editar = subir versión,
   nunca in place; digest del conjunto sellado en cada decisión.

3. **Consentimiento-por-política** (ADR-0042 propuesta; extiende ADR-0034). El
   humano aprueba **una vez** una política de divulgación (campos, proveedor,
   **presupuesto de llamadas/tokens/gasto**). Cada invocación se audita contra
   ella; agotado el presupuesto → lo ambiguo restante va a `revisar/` (degrada,
   no bloquea, no dispara la factura). La clave sigue en el almacén del SO.

4. **`df-agent`** (ADR-0044 propuesta). Orquesta el ciclo de vida: intención →
   plan completo → `df-rules` → congelar → ejecutar → verificar → informe. Con
   **presupuestos** (tokens, divulgación, tiempo, operaciones), **cortacircuitos**
   ("para y manda a revisar en bloque si la tasa de ambigüedad supera X%") y
   **modo dry-run** (produce el plan + qué reglas dispararían, sin ejecutar).

### El gate autónomo y su procedencia

Una **aprobación autónoma** sella, en la misma transacción inmutable del
congelado del manifiesto, la **procedencia completa**: digest del plan; id +
digest del conjunto de reglas que autorizó; veredicto y confianza recalculada;
digest de la política de divulgación; proveedor de modelo por item; evento en el
ledger encadenado. Así "¿por qué el agente colocó/aprobó esto?" tiene respuesta
determinista. El congelado del manifiesto ocurre **igual**; solo cambia que el
autorizador es regla+agente en vez de humano, y esa procedencia queda sellada.

### Trazabilidad total

Cada archivo deja registrado su recorrido en SQLite: **ruta origen (cruda) →
ruta destino o `revisar/` + motivo + regla + confianza + proveedor**. Ese mapa
completo es el **informe final exportable** y es lo que permite (a) confiar en un
run que no se vio y (b) **deshacerlo** por completo (reconstruir el mapeo para
revertir), reforzando que todo es reversible.

Además, el informe **abre con un esquema de organización** legible: en Modo 1,
qué réplicas se colapsaron en qué ramas canónicas; en Modo 2, la taxonomía
inventada y el porqué de cada rama. Cuando el agente **inventa**, no te deja
frente a una estructura misteriosa sino **justificada y auditable**.

### Impacto en las garantías absolutas (RFC-0001)

| Garantía | Efecto | Por qué sigue intacta |
| --- | --- | --- |
| Origen inmutable | Ninguno | Mismas ops de fachada; executor/verifier jamás abren orígenes para escritura |
| Sin borrado/sobrescritura | Ninguno | Copia-solo; destino vacío; colisiones desambiguadas, nunca sobrescritas |
| SQLite única verdad | Reforzado | Toda decisión, procedencia y mapa origen→destino se persisten |
| Clientes solo vía fachada | Reforzado | La superficie de herramientas ES la fachada; no hay canal alterno |
| Migraciones inmutables | Ninguno | Se **añade** migración append-only; no se edita ninguna aplicada |
| Verificación independiente | Ninguno | El verificador sigue re-leyendo y re-hasheando sin confiar en agente ni executor |
| No adelantar milestones | Respetado | Milestone nuevo, post-1.0 |

La autonomía cambia **quién autoriza**, no **qué garantiza el motor**.

### Impacto en formatos versionados y SQLite

- Nueva **migración append-only**: procedencia de aprobación autónoma; política
  de divulgación y su digest; auditoría de invocación del agente; mapa
  origen→destino con motivo. Numeración a continuación de la última aplicada.
- Nuevos identificadores de contrato congelados (test `frozen_contracts`):
  esquema de reglas, esquema de política, esquema de plan del agente.

### Impacto en seguridad y modelo de amenazas

- **A1 — Modelo hostil que induce un plan dañino.** La IA no es autoridad;
  `df-rules` fail-closed decide; confianza recalculada localmente; fronteras
  duras; destino vacío; todo reversible; lo dudoso a `revisar/`.
- **A2 — Prompt-injection desde el contenido inventariado.** Redacción por
  defecto, esquema de salida cerrado, corpus adversarial ampliado a nivel-plan.
- **A3 — Exfiltración por divulgación autónoma.** Consentimiento-por-política con
  presupuesto y campos acotados; ruta local por defecto; auditoría append-only.
- **A4 — Deriva silenciosa del conjunto de reglas.** Reglas versionadas y con
  checksum; digest sellado; rechazo al abrir si el checksum no cuadra.
- **A5 — Agente en bucle o gastando presupuesto en basura.** Cortacircuitos y
  presupuestos duros; degradación a `revisar/`.

## Alternativas

- **Escalado bloqueante a un humano durante el run.** Descartado: en discos
  lentos detiene el trabajo horas o días. La ambigüedad se resuelve con una
  **acción de reserva determinista** (`revisar/`), no bloqueando.
- **Dar al agente autoridad directa sobre el gate (sin reglas).** Descartado:
  convierte la seguridad en confianza en el modelo; no auditable.
- **`revisar/` agrupado por motivo.** Descartado como eje principal: pierde el
  contexto posicional. El motivo se conserva como metadato + informe.
- **Solo-local / Nube-pura.** Ambos son configuraciones de política, no la única
  vía: por defecto local-first con escalado a nube acotado por presupuesto.

## Plan de adopción

Orden obligado (los pasos 1–2 constituyen un L1 seguro y entregable):

1. **Superficie de herramientas sobre la fachada** (API in-process + MCP).
2. **`df-rules` + esquema de procedencia auditable + migración append-only.** Es
   el ~70 % del trabajo de L2 y lo que hace segura la retirada del humano.
3. **Consentimiento-por-política** (extensión de ADR-0034).
4. **`df-agent`** con plan-congelado, `revisar/` espejo, robustez de disco viejo,
   presupuestos, cortacircuitos, dry-run e informe origen→destino.
5. **Subida gradual de la confianza de auto-colocación por dominio**, empezando
   por el de menor ambigüedad (**deduplicación exacta a destino vacío**) y
   ampliando con evidencia (modo sombra para calibrar umbrales sobre el corpus
   real antes de confiar).

Compatibilidad: puramente aditivo. L0 (asistida) sigue disponible sin cambios.

## Preguntas abiertas

1. **Lenguaje de las reglas.** ¿DSL declarativo propio, tabla de condiciones
   versionada en SQLite, o política en Rust compilada con test de contrato?
   (editabilidad por el usuario ↔ seguridad/auditabilidad).
2. **Calibración de confianza.** ¿Un modo "sombra" que coloca todo en el espejo
   de `revisar/` y compara con la decisión humana para fijar umbrales?
3. **Dominio de arranque de L2.** Deduplicación exacta a destino vacío como
   primer dominio; ¿qué sigue?
4. **Escritorio.** ¿Cómo se presenta la intención capturada (Fase A), el progreso
   de un run largo y el informe origen→destino en la UI de Tauri?
5. **Reproducibilidad con nube.** Un modelo de nube no es determinista; ¿bastan
   los ids de campo estables para que la auditoría siga siendo verificable?
6. **Política de colisión de nombres por defecto.** ¿Sufijo por hash de contenido
   siempre, o a `revisar/_colision/` cuando además hay duda de ubicación?
