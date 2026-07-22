# Diseño propuesto: ejecución estricta paralela (M1.0.1)

Estado: **implementación iniciada por incrementos** (ver
[§10 Estado de implementación](#10-estado-de-implementación-coordinación) al
final). El diseño fue revisado y se juzgó sólido; la construcción va desde el
modelo de recuperación, con todas las ventanas de caída documentadas. Nada aquí
relaja el modo estricto; la concurrencia es un *cómo*, no un *qué*.

## 1. Por qué (cuello de botella medido)

Del [baseline](m1.0.1-baseline.md), corpus A (100k archivos, 111.060 ops):
copiar bytes es el **5,7 %** del tiempo de ejecución. Domina la latencia por
archivo: **~32 % en tres commits SQLite por operación** (lease, claim,
result) y **~32 % en syscalls por archivo** (create_new, sync_all, rename).
En un NVMe que hace GB/s, la ejecución rinde 0,5 MiB/s: es serialización, no
ancho de banda.

Dos palancas, ambas sin tocar garantías:

1. **Solapar la latencia de filesystem** entre archivos con varios workers.
2. **Agrupar los commits SQLite** del coordinador en microlotes que preserven
   la recuperación por operación.

## 2. Modelo de concurrencia

```
            ┌────────────────────────── coordinador (único dueño de SQLite) ──┐
 batch ──►  │ 1. reservar lote de ops + tokens de lease (1 txn)               │
            │ 2. despachar (op, token) por canal acotado ─────────┐          │
            │ 5. recibir identidad física del parcial ◄──────────┐│          │
            │ 6. persistir claims en microlote (1 txn) ──ACK────► ││ workers  │
            │ 9. recibir resultado ◄─────────────────────────────┘│ (N)      │
            │ 10. persistir resultados en microlote (1 txn)        │          │
            └──────────────────────────────────────────────────────┘          │
                         ▲ backpressure: máx. `in_flight` ops sin persistir
```

- **Un solo escritor SQLite**: el coordinador. Los workers **nunca** abren la
  base (regla 15). Ya es cierto en hash y verify (M1.0.1); aquí se extiende a
  execute, que hoy escribe SQLite desde el hilo de trabajo.
- **Workers dedicados al filesystem**: `create_new`, copia+doble hash,
  `sync_all`, comprobaciones de origen/frontera, finalize. Cada uno con su
  buffer reutilizable (ya existe la pieza).
- **Canales acotados + backpressure**: como máximo `max_in_flight` operaciones
  vivas sin resultado persistido. Evita memoria proporcional al nº de archivos
  (rule: ningún buffer O(nº archivos)).
- **Cancelación y shutdown limpio**: una bandera atómica; los workers terminan
  la op en curso, no arrancan otra; el coordinador drena y persiste lo hecho.

## 3. El protocolo por archivo NO cambia

Cada operación conserva las 10 etapas del §27.1 en el mismo orden
(RFC-0001, ADR-0017/0018/0021):

1. lease durable (coordinador, antes de crear nada);
2. `create_new` del parcial (worker);
3. identidad física capturada del handle abierto (worker);
4. claim durable de esa identidad (coordinador, **antes** de copiar);
5. copia + SHA-256 + BLAKE3 (worker);
6. `sync_all()` estricto (worker);
7. re-comprobación del origen (worker, §14.5);
8. revalidación de fronteras (worker);
9. finalize no-replace (worker);
10. resultado durable (coordinador).

La única diferencia es **quién** ejecuta cada etapa. Las dos etapas que
tocan SQLite (1, 4, 10) las hace el coordinador; el resto, el worker. La
etapa 4 (claim) sigue siendo una **barrera durable**: el worker no copia
hasta que el coordinador confirma que la identidad del parcial quedó
persistida. Así la propiedad del parcial sigue ligada a su identidad física
(regla 7) y un token o nombre por sí solos nunca autorizan borrar.

## 4. Exclusión por destino (nuevo, obligatorio)

Con varias ops en vuelo hay que impedir carreras entre destinos. El
coordinador mantiene un **conjunto de claves de destino ocupadas** y no
despacha una op cuyo destino colisione con otra viva. Dos ops no pueden
trabajar a la vez sobre:

- el mismo destino final;
- la misma ruta de parcial;
- un destino y otro cuyo **sufijo de colisión determinista** (§27.3) pueda
  coincidir — se reservan por el *stem* base, no por el nombre final;
- rutas en relación padre/hijo que puedan crear una carrera de `create_dir`.

Las operaciones `CREATE_DIRECTORY` se ejecutan en una **etapa previa
secuencial** (o serializadas por prefijo): baratas, y así el árbol existe
antes de que las copias corran en paralelo (evita carreras de creación de
directorio padre).

## 5. Ventanas de caída (todas, con recuperación)

El modelo de recuperación **por operación** actual ya cubre estas ventanas;
la versión paralela las preserva exactamente. `T` = token de lease durable,
`I` = identidad física del parcial (claim durable).

| # | Ventana (justo después de) | En disco | En SQLite | La siguiente ejecución | Puede borrar | Prueba de propiedad |
| --- | --- | --- | --- | --- | --- | --- |
| A | reservar token, antes de crear parcial | nada | op RUNNING + `T`, sin claim | reclaim no ve parcial; re-lease `T'` y reintenta | — (no hay parcial) | n/a |
| B | crear parcial, antes de persistir claim | parcial con `T` en el nombre, identidad `I₀` | op RUNNING + `T`, **sin** claim | parcial sin claim = huérfano no reclamado; se deja intacto, se re-lease `T'` y el nuevo parcial usa `T'` | **no** (sin claim durable no hay prueba de propiedad) | ninguna todavía |
| C | persistir claim, antes/durante copia | parcial `T`, identidad `I` | op RUNNING + `T` + claim `I` | reclaim: si existe parcial con nombre `T` **e** identidad `I` coincide → es nuestro, se borra y se reintenta | **sí**, solo si nombre `T` **y** identidad `I` coinciden | claim durable `I` |
| D | `sync_all`, antes de finalize | parcial completo `T`/`I`, datos en disco | RUNNING + claim `I` | igual que C: parcial reclamable por `T`+`I`, se borra y reintenta (recopia) | sí (T+I) | claim `I` |
| E | finalize (rename hecho), antes de result | destino final con datos; parcial ya no existe | RUNNING + claim `I` (¡result no!) | reclaim no encuentra parcial `T`; el destino existe con el hash esperado → colisión §27.3 se resuelve como `SKIP_REPRESENTED` (idempotente); result se persiste | no borra el destino final | destino = contenido esperado |
| F | persistir result, terminal | destino final | op COMPLETED | nada que hacer; op no se re-ejecuta | — | — |

Claves de seguridad que se mantienen:

- **Entre B y C** un parcial existe sin claim: es intocable. Solo un claim
  durable (identidad capturada del handle que ganó `create_new`) autoriza
  borrarlo. Un `RUNNING` sin claim, o un nombre/token sin identidad, **nunca**
  autorizan borrado (idéntico al modelo actual; probado por los tests
  `crash_after_create_before_claim_preserves_the_unclaimed_orphan` y
  `a_partial_substituted_after_claim_is_never_deleted`).
- **Caída del coordinador con workers vivos**: los workers son procesos del
  mismo binario; al morir el proceso mueren los handles. Al reabrir, cada op
  RUNNING se reevalúa por su ventana (A–F). Ninguna respuesta tardía de un
  worker se acepta tras el reinicio porque el coordinador que la esperaba ya
  no existe.

## 6. Microlotes de commit: qué se puede agrupar y qué no

Se pueden agrupar en una transacción los **claims** de varias ops y los
**results** de varias ops, **siempre que** el orden preserve la recuperación
por operación: una op solo avanza a copiar tras ver su claim durable, y solo
se marca COMPLETED tras su finalize. Agrupar N claims en una txn significa que
esos N parciales quedan reclamables juntos; si la txn no llega, esos N siguen
como huérfanos no reclamados (ventana B) — seguro.

**No** se agrupa si con ello se pierde la reclamabilidad por operación: p. ej.
persistir un result antes de que su finalize haya ocurrido rompería E→F. El
coordinador ordena: claim(microlote) → [workers copian/finalizan] →
result(microlote de los ya finalizados). Un result solo entra al lote cuando
su finalize está confirmado por el worker.

## 7. Respuestas de worker: aceptación estricta

Una respuesta de worker se acepta solo si concuerda con la op esperada:
`(operation_id, run_id, lease_token, estado esperado, identidad esperada)`.
Una respuesta tardía de un lease anterior, o duplicada, se **rechaza o trata
idempotentemente** (el estado ya terminal de la op la ignora). Esto se prueba
con inyección de: respuesta duplicada, respuesta tardía de lease previo,
worker en pánico, worker que no responde (timeout → la op queda RUNNING y se
recupera por su ventana).

## 8. Perfiles de durabilidad (ver ADR propuesto)

- **`strict`** (actual, por defecto): `synchronous=FULL`, `sync_all` por
  archivo, recuperación por operación, verificación independiente.
- **`strict-parallel`**: **mismas garantías** que `strict`, con la concurrencia
  de este diseño. No se declara equivalente hasta pasar los tests de caída de
  §5 e igualar bit a bit la salida y los hashes frente a `workers=1`.
- **`fast`** (opcional, separado): puede omitir `sync_all` individual y confiar
  en el page cache; conserva hashes, no-overwrite y verificación; **no** puede
  afirmar persistencia física ante corte de energía inmediato. Selección
  explícita, registrado en el ledger, visible en informes, con aviso, nunca
  automático, nunca en perfiles evidenciales sin decisión explícita. Requiere
  ADR de durabilidad.

## 9. Criterios de aceptación antes de implementar el bucle paralelo

1. Los 28 tests adversariales/recuperación del executor pasan sin cambios.
2. Nuevos tests de inyección de fallo para las ventanas A–F y para respuestas
   de worker tardías/duplicadas/en pánico.
3. `workers=1` reproduce exactamente la salida secuencial (mismo destino,
   mismos hashes, mismos resultados, mismo estado final).
4. Comparación determinista `workers=1` vs `workers=N` sobre corpus real.
5. Ganancia medida ≥2× en corpus de archivos pequeños sin regresión >5 % en
   secuencial ni en archivos grandes, memoria acotada (< 512 MB en 1M).

Hasta cumplir 1–5, `strict-parallel` no se ofrece como predeterminado y
`strict` sigue siendo el modo de los proyectos existentes y evidenciales.

## 10. Estado de implementación (coordinación)

Nota entre agentes para no duplicar trabajo (como pasó con el paralelismo de
hash/verify). La implementación avanza en **incrementos verdes y commiteados**;
cada uno deja el árbol compilando y con tests en verde.

| Inc | Trabajo | Estado |
| --- | --- | --- |
| 1 | Partir `copy_file` en `prepare_copy` (sin SQLite) → **barrera de claim** → `finish_copy` (sin SQLite). Aísla la costura coordinador/worker; comportamiento idéntico | ✅ hecho — commit `44bc009`, 28/28 tests |
| 2 | `ExecuteOptions.workers`/`max_in_flight` + pre-stage secuencial de `CREATE_DIRECTORY` (`run_directory_stage`) + módulo de **exclusión por destino** (`dest_exclusion::DestinationGuard`, puro, unit-tested) | ✅ hecho — 31/31 tests; scaffolding `#[allow(dead_code)]` hasta que el Increment 3 lo cablea |
| 3 | **Pool de workers acotado + protocolo coordinador↔worker** con la barrera de claim y backpressure. `workers=1` ≡ secuencial | ✅ hecho — `run_parallel` (coordinador dueño de SQLite + workers `std::thread::scope`, barrera de claim por canal, `DestinationGuard`, dir pre-stage). Default `execute` sigue secuencial (opt-in hasta Increment 5). Test end-to-end `parallel_execution_matches_sequential_byte_for_byte` (workers=1 vs 8, salida byte-idéntica); 32/32 tests estables en 3 corridas |
| 4 | **Microlotes** de commit lease/claim/result (el ~32 % de SQLite) | ⬜ pendiente |
| 5 | Tests de inyección de fallo para las ventanas A–F + respuestas tardías/duplicadas/en pánico + determinismo `workers=1` vs `N` (§9.1–9.4) | ◐ parcial — determinismo end-to-end ya cubierto (Increment 3); faltan las inyecciones de fallo por ventana A–F y respuestas tardías/en pánico bajo el pool |
| 6 | Perfiles de durabilidad `strict`/`strict-parallel`/`fast` + ADR de durabilidad (§8) | ⬜ pendiente |
| 7 | Medir sweep de `execute` + documentar en `m1.0.1-results.md` + PR de borrador | ⬜ pendiente |

**Quien continúe: retomar desde el Increment 4 (microlotes) o completar el
Increment 5 (inyección de fallo A–F).** El Increment 3 dejó `run_parallel`
funcionando y probado byte-idéntico al secuencial. Rutas de trabajo abiertas:
- **Increment 4**: agrupar lease/claim/result en microlotes (el ~32 % de
  SQLite serializado). Es la palanca que falta para que los archivos pequeños
  ganen de verdad; hoy `run_parallel` commitea por operación como el
  secuencial, así que solapa filesystem pero no reduce el nº de commits.
- **Increment 5**: tests de inyección de fallo por ventana A–F y respuestas
  tardías/duplicadas/en pánico bajo el pool, antes de considerar mover el
  default de `execute` a paralelo.
- **Increment 7**: sweep de `execute --workers` y resultados.
