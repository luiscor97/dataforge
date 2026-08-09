# Estructura de M2.4 a M2.6

**Fecha:** 2026-08-01
**Estado:** Plan de ejecución, no diseño. El diseño está en
[RFC-0002](../rfcs/RFC-0002-autonomy-ladder.md) y en ADR-0041, 0042 y 0044.

Documento para retomar el trabajo. Dice **qué crate, qué tipos, qué contratos se
mueven y qué prueba cada cosa**, en el orden en que hay que hacerlo. No repite
el porqué: para eso están las ADR.

> **No hay crates vacíos.** Se escribió antes de tocar código, precisamente
> para no crear esqueletos: un crate que compila y no hace nada es
> funcionalidad simulada (CONTRIBUTING) y adelanta hito (regla 7).
>
> **Actualizado el 2026-08-01:** los tres hitos tienen ya su primera pieza
> puesta, y ninguna es un esqueleto — cada una entró con su comportamiento y sus
> tests. Lo que sigue pendiente en cada apartado es cableado y persistencia, no
> tipos.

## Lo que bloquea, hoy

| Bloqueo | A quién afecta | Estado |
| --- | --- | --- |
| Precedencia recomendación vs. prueba (ADR-0045) | M2.3, y de rebote el valor de M2.4 | **Abierto — decisión del usuario** |
| Lenguaje de reglas (RFC-0002 P1) | M2.4 | **Resuelto en ADR-0041**: motor en Rust, parámetros en tabla versionada |
| Calibración de confianza (P2), dominio de arranque (P3) | M2.6 paso 5 | Abierto, no bloquea empezar |
| Presentación en escritorio (P4) | M2.6 | Abierto, no bloquea el motor |

El primero importa más de lo que parece: `df-rules` decide sobre planes, y si
una recomendación sigue anulando la disposición de duplicados, `df-rules`
heredará el mismo techo que ADR-0045 encontró. **Conviene cerrarlo antes de
M2.4**, no durante.

---

## M2.4 — `df-rules`

El ~70% del trabajo que hace segura la retirada del humano. Crate nuevo,
`crates/df-rules`, dependiendo solo de `df-domain`, `df-error` y `df-ledger`
(no de `df-db`: no hace E/S).

> **Puesto ya (2026-08-01):** el crate, `Verdict`, las cuatro fronteras duras,
> `RuleParams` con digest y validación de signos, y `evaluate`. 13 tests.
> `RULE_SET_SCHEMA_VERSION` y `HARD_BOUNDARY_COUNT` en `frozen_contracts`.
>
> **Falta:** persistir `rule_sets` (migración 0021) y conectar los pesos al
> `location_cost` que hoy usa constantes en `df-db`. La clasificación de estado
> de injerto (§3) espera a la precedencia de ADR-0045.

### Superficie

```
Verdict          = Authorize | Review | Deny
RuleOutcome      { verdict, rule_set_id, rule_set_digest, score, evidence }
RuleSet          { id, version, digest, params: RuleParams }
RuleParams       { representative: RepresentativeWeights,
                   thresholds: AutoApprovalThresholds,
                   context_penalties: Vec<ContextPenalty> }
evaluate(plan_view, rule_set) -> Vec<RuleOutcome>   // una por operación
```

`evaluate` no toca la base ni el disco: recibe evidencia ya leída y devuelve
veredictos. Eso es lo que permite probarlo con tablas y lo que impide que el
gate dependa de E/S.

### Los parámetros, como datos

Migración **0021**, tabla `rule_sets` (id, version, digest, params JSON,
created_at) + `rule_set_params` si conviene normalizar. Misma disciplina que
las migraciones: **editar es subir versión, nunca in place**, y el digest se
verifica al abrir — si no cuadra, se rechaza (ADR-0041 §2, deriva A4 del modelo
de amenazas).

Los tres pesos del score de representante salen de la fórmula real del trabajo
original y son parámetros, no constantes:

```
score = −8·profundidad − 1,1·longitud_ruta + 15·(mtime más antiguo)
```

El signo del tercero es lo importante y conviene no perderlo de vista: en
duplicados **byte-idénticos** el original conserva su fecha vieja y la copia
recibe una nueva al copiarse, así que **el más antiguo es el original**. Entre
*variantes* de distinto hash es al revés, y por eso esa regla no se aplica ahí.

### Fronteras duras, en código y no en la tabla

Son invariantes, no parámetros, y van *fail-closed* (ADR-0041 §4):

- nunca deduplicar por nombre (mismo nombre + hash distinto = documentos
  distintos);
- evidencia compartida entre asuntos → siempre `Review`;
- destino vacío obligatorio; nunca borrar ni sobrescribir;
- perfil de dominio protegido → `Review` por defecto, jamás auto-colapsar.

Un test por frontera, y cada uno debe fallar si alguien mueve la frontera a la
tabla de parámetros.

### Contratos que se mueven

`RULE_SET_SCHEMA_VERSION` entra en `frozen_contracts` en el mismo commit, y la
migración 0021 sube el recuento a 21 (ADR-0037 §2).

### Cómo saber que está hecho

- Los cuatro estados de injerto de ADR-0041 §3 enrutan como dice la tabla.
- Un conjunto de reglas con digest alterado **no abre**.
- Cada veredicto lleva el id de la regla que lo determinó — sin eso, "¿por qué
  se autorizó esto?" no tiene respuesta y el hito no está hecho.

---

## M2.5 — Consentimiento por política

Extiende ADR-0034. No es un crate nuevo: vive donde ya vive el consentimiento,
en `df-ai` y su almacén, más la auditoría en `df-db`.

> **Puesto ya (2026-08-01):** `df-ai::policy` — `DisclosurePolicy`, `Budget`,
> `Consumption`, `authorize`, con digest canónico. 11 tests.
> `DISCLOSURE_POLICY_SCHEMA_VERSION` en `frozen_contracts`.
>
> **Falta:** persistir política y auditoría de consumo (migración 0022) y
> conectar `authorize` a la ruta de transporte, que sigue usando el token por
> petición de ADR-0034.

### Superficie

```
DisclosurePolicy { fields: Vec<FieldId>, provider, budget: Budget, digest }
Budget           { calls, tokens, bytes, spend_cents }
Ledger de consumo: cada invocación descuenta y queda auditada
```

> **Decisión pendiente antes de la migración 0022:** el tipo actual de
> `df-ai::policy` contiene llamadas, bytes divulgados y gasto, pero no tokens.
> La documentación y el contrato persistido deben elegir la misma superficie;
> no se congela la discrepancia en una migración.

### Las tres reglas que lo hacen seguro

1. **Se congela y se sella el digest** al aprobarla, como el manifiesto de
   ejecución. Cada invocación lleva ese digest en su procedencia.
2. **Se audita antes de tocar clave o red.** Una invocación que exceda campos o
   presupuesto se rechaza *antes*, y el rechazo queda en la auditoría
   append-only. Auditar después no sirve: la divulgación ya ocurrió.
3. **Presupuesto agotado → degradar, no bloquear.** Lo ambiguo restante va a
   `revisar/`. Nunca se detiene el run ni se dispara la factura.

La clave sigue en el almacén del SO. Nunca en SQLite, nunca en la política.

### Contratos

Migración **0022**: política, su digest, y la auditoría de invocación.
`DISCLOSURE_POLICY_SCHEMA_VERSION` a `frozen_contracts`.

### Cómo saber que está hecho

- Una invocación fuera de presupuesto no llega a la red — probado
  interceptando el transporte, no confiando en el contador.
- Agotar el presupuesto a mitad de run deja el run **terminando**, con el resto
  en `revisar/`.

---

## M2.6 — `df-agent`

El bucle completo. Crate nuevo `crates/df-agent`, y **conduce el motor por
`df-tools`**, no por la fachada directamente: si el agente propio se saltara su
propia superficie, la frontera de M2.1 no probaría nada.

> **Puesto ya (2026-08-01):** la lógica de decisión sin E/S — `Stage` con orden
> obligado, `AgentBudget`, `RunTally`, `assess`, `RunMode`. 11 tests, incluido
> `the_loop_can_never_block`, que es donde habría que defender cualquier
> variante futura que espere a un humano.
>
> **Falta:** conducir el motor por `df-tools`, pre-vuelo de espacio, los buckets
> técnicos, reanudación desde el manifiesto e informe origen→destino.

### El ciclo

```
intención → inventario → pasada 1 (dedup+representante, sin IA)
          → pasada 2 (árboles injertados) → diagnóstico de modo
          → plan completo → congelar → [dry-run] → ejecutar
          → verificar → informe
```

Las pasadas 1 y 2 resuelven el grueso **sin IA**. El modelo se reserva para la
cola ambigua y para el Modo 2 (inventar taxonomía), bajo la política de M2.5.

### Lo que no es negociable

- **No bloquea jamás.** Duda, ambigüedad o frontera dura → `revisar/` y sigue.
- **Reanudable** desde el manifiesto congelado + el ledger (ADR-0029). Lo ya
  colocado es no-op por identidad física.
- **Presupuestos y cortacircuitos**: tokens, divulgación, tiempo, operaciones,
  y "para y manda a `revisar/` en bloque si la ambigüedad supera X%".
- **La autoridad es `df-rules`.** `df-agent` propone y ejecuta; el veredicto lo
  sella el gate.

### Aquí aterrizan los buckets que M2.2 dejó pendientes

`revisar/_ilegible/` y `revisar/_verificacion-fallida/` se declaran **cuando
existe quien los escribe**, que es este hito: error de lectura del origen y
fallo de verificación. `revisar/_sin-ubicar/` llega antes, con la clasificación
de M2.3. Declararlos ahora habría sido crear carpetas que nada usa.

Y el pre-vuelo de espacio: comparar tamaño planificado contra libre en destino
**antes** de empezar. No es una mejora — llenar el disco a mitad de un trabajo
de dos días es el fallo más caro que este producto puede tener.

### Contratos

Migración **0023**: procedencia extendida del gate autónomo (regla, política,
confianza, proveedor) y el mapa origen→destino exportable.

---

## Orden y por qué

1. **Cerrar la precedencia de ADR-0045.** Barato, y sin ello M2.4 hereda el
   mismo techo.
2. **M2.4.** Todo lo demás delega autoridad en él.
3. **M2.5.** `df-agent` no puede llamar a un modelo sin presupuesto que lo
   acote.
4. **M2.6.** Necesita a los dos anteriores.

M2.3 (clasificación) corre en paralelo a M2.4: no dependen entre sí, y es la
precondición de la deduplicación — 234,2 GB de los 239,7 GB de redundancia
siguen bloqueados por ella.

## La prueba final

Ninguno de estos hitos se declara hecho por tener su crate. La 2.0 se declara
terminada cuando reproduce sola, sobre el corpus real, los nueve umbrales de la
definición de hecho de [ROADMAP-2.0](ROADMAP-2.0.md). Un run que los cumpla con
un humano en el gate es un L1 entregable y un punto de parada legítimo.
