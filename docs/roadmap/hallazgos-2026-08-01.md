# Hallazgos y plan — 2026-08-01

**Estado:** Notas de trabajo, no decisión. Ninguna ADR sale de aquí todavía.

Documento de traspaso. Recoge lo que se midió y lo que se concluyó en una
sesión, para que no dependa de la memoria de nadie. Si retomas el repo, lee
esto después de [estado-superficie-agentica.md](estado-superficie-agentica.md).

---

## 1. Lo que se midió (hechos, no estimaciones)

Corpus sintético de 20.000 ficheros, 60% duplicados → 4.770 conjuntos. Pipeline
real, y JSON-RPC contra el binario `df-mcp` de verdad.

| Herramienta | bytes de respuesta |
| --- | --- |
| `duplicate_report` | **5.976.389** |
| `structural_review_queue` | 88.833 |
| `project_status` | 2.281 |
| `tree_clone_report` | 225 |
| `context_report` | 213 |
| **`structural_review_classes`** | **833** |

**~1.245 bytes por conjunto de duplicados.** Extrapolado a los 28.537 conjuntos
del archivo real: **~35 MB en una sola respuesta**, del orden de 10 millones de
tokens.

Eso no es «caro con un modelo barato». Es **unas 50 veces la mayor ventana de
contexto que existe**, así que ese camino no es viable con ningún modelo. La
herramienta correcta para la misma pregunta —`structural_review_classes`— es
**7.175 veces más pequeña**.

`duplicate_report` y `structural_review_queue` **no tienen cota ni paginación**:
los únicos `LIMIT` de esas consultas son subconsultas correlacionadas para
«última decisión».

**Corrección de una suposición:** se dijo que el `to_string_pretty` de `df-mcp`
costaba «el doble de tokens». Medido: compacto 5.514.209 vs pretty 5.937.841,
es decir **7,7%**. La carga está dominada por cadenas largas (hex y rutas), no
por espaciado. Compactar sigue siendo correcto y es marginal.

**Coste por fichero, medido:** crear + escanear + hashear + analizar 20.000
ficheros tardó **7,06 s**. Extrapolado a 158.219 ficheros: menos de un minuto.
El cuello del pipeline son los bytes, no los ficheros.

## 2. La procedencia del score de representante es desconocida

```
score = −8·profundidad − 1,1·longitud_ruta + 15·(mtime más antiguo)
```

ADR-0041 dice que la fuente de verdad es `revision_documental.csv`, columna
`auto_reason`, de la ejecución original.

**Esa ejecución no fue criterio humano.** Fue Codex conduciendo de forma
autónoma durante diez días, sin auditoría. Los tres pesos son, con toda
probabilidad, algo que el modelo escribió a mitad de run. **Nadie los ha
justificado ni validado nunca.**

El 2026-08-01 esos números entraron en `crates/df-rules` con validación de
signos, digest y test de contrato — lo que les da apariencia de fórmula
establecida. **Eso hay que corregirlo en el doc-comment**: que diga de dónde
vienen y que su procedencia es desconocida. La deuda es de quien los metió.

## 3. El producto es una formalización de una improvisación no auditada

Conviene separar dos cosas, porque no tienen el mismo fundamento:

| Capa | Origen | ¿Sólida? |
| --- | --- | --- |
| **Garantías** — origen inmutable, solo copia, verificación independiente, ledger | Primeros principios | **Sí** |
| **Evidencia** — hash, firmas Merkle, contención de árboles, cobertura | Primeros principios | **Sí** |
| **Criterio** — qué duplicado sobra, qué es «contexto», los pesos, los buckets | Arqueología de los diez días | **No** |

El chasis es bueno. El criterio es memoria de algo que nadie puede describir.

**Observación que merece no perderse:** el principio rector de RFC-0002 —*«la IA
nunca es la autoridad»*— es exactamente la corrección de cómo nació el producto.
En los diez días la IA *fue* la autoridad, sin auditoría. Que su autor no pueda
explicar qué ocurrió no es un descuido: es el síntoma que el RFC ya diagnosticó.

**Qué hacer con ello:** no ajustar reglas contra el resultado de los diez días —
sería destilar la improvisación de un modelo en reglas deterministas, con más
apariencia de rigor. Auditar en cambio una **muestra estratificada por clase**
(cientos de casos, no 158.219; barato porque 3.702 de 5.334 son la misma clase).
Eso sí da un patrón de referencia real, pequeño y con conocimiento de dominio
dentro.

## 4. Replicabilidad: lo que generaliza es el perfil

La evidencia es ciega al dominio y generaliza perfectamente. La **utilidad** no:
con perfil `generic` sobre el archivo real, 36.381 de 36.459 carpetas salieron
`NEUTRAL`, sin fronteras protegidas, y **el 63% del volumen fue a revisión**.
Ésa es la respuesta correcta de un motor sin conocimiento de dominio, y deja el
producto en «encuentra duplicados exactos», no en «ordena el caos».

Por tanto **replicar a otro entorno = escribir un perfil**, y la pregunta útil es
quién lo escribe.

**Reencuadre propuesto — el modelo no clasifica ficheros, propone un perfil:**

| | Modelo juzga cada fichero | Modelo propone un perfil |
| --- | --- | --- |
| Decisiones del modelo | 158.219 | **1** |
| Auditable | No | Sí, ~50 líneas |
| Reproducible | No | Sí, determinista al aplicarlo |
| Corregible | Rehacer todo | Cambiar una línea y reejecutar |
| Reutilizable | No | Sí, entre clientes del mismo dominio |

Es la tesis de RFC-0002 aplicada a la Fase A: **la IA propone, la regla
determinista decide.** Y redefine M2.3, que hoy está escrito como «que el motor
clasifique», a **«el motor sabe aplicar un perfil rico; el agente sabe proponer
uno»** — que además es más barato.

Ejemplo que lo ilustra: para un fotógrafo, `df-media` ya extrae resolución y
duración **y no alimenta ninguna decisión**. Ahí no falta motor, falta política.

## 5. Arquitectura: hay dos relojes y están fusionados

- **Reloj de decisión:** segundos. Con la tabla de hashes hecha, análisis, plan y
  decisiones son cómputo puro (medido: 20.000 ficheros en 7 s).
- **Reloj de bytes:** horas. Hashear y copiar 443,9 GB.

Casi todos los problemas vienen de tenerlos en el mismo proceso: `df-mcp`
bloquea porque sirve a los dos; el límite de sesión duele porque el reloj de
bytes sobrevive a la sesión; la respuesta de 5,98 MB es una herramienta de
decisión devolviendo datos a escala de bytes.

**Separación propuesta:** conversación (`df-mcp`, nunca bloquea, siempre pagina)
y trabajo (worker desacoplado, dueño del reloj de bytes) que **no se hablan
entre sí**, solo a través de SQLite. Es ADR-0043 §5 llevado a su conclusión: sin
estado de sesión, cualquier sesión posterior retoma leyendo la misma verdad.

**Flujo que sale de ahí:** hablar (intención) → esperar (hash, desacoplado) →
**hablar (análisis, plan y decisiones: minutos, todo instantáneo)** → aprobar →
esperar (copia y verificación) → hablar (informe). Dos tramos desatendidos y
tres conversaciones cortas.

**Precondición dura:** un **registro de vitalidad** (pid, host, fase, latido).
Sin él, desacoplar empeora un problema que el código ya reconoce — *«el motor no
puede distinguir un run muerto de uno vivo sosteniendo la misma base»*, que es
por lo que `hash --resume-interrupted` es hoy una afirmación del operador y no
una comprobación.

**Lo que NO hay que hacer:** un daemon de larga vida (el repo ya tiene tres
workers aislados; `apps/daemon` puede seguir vacío), fusionar hash y copia
(ahorra ~4 h y cuesta la aprobación previa, la reanudación y la idempotencia), y
verificar desde el buffer de escritura (no comprueba el disco).

**Mejora que aparece sola:** con la ejecución ya desacoplada, verificar cada
fichero justo después de copiarlo sigue siendo relectura independiente y se
lleva por delante casi toda la cuarta pasada de E/S.

## 6. ETA del run sobre el archivo real

Dominado por E/S, **no por el modelo**. Cuatro pasadas del tamaño del archivo
(hash, lectura de copia, escritura de destino, relectura de verificación) ≈
**1,8 TB de E/S**:

| Disco | ETA |
| --- | --- |
| USB 2.0 / HDD externo viejo (~30 MB/s) | ~17 h |
| USB 3.0 / SATA (~100 MB/s) | ~5 h |
| SSD interno (~500 MB/s) | ~1 h |

Los diez días **no eran esto**: eran criterio, no disco. Eso es lo que la 2.0
elimina.

---

## Plan propuesto, por orden

**Reparar primero (10 minutos).** El doc-comment de los pesos en `df-rules`, para
que no aparenten un rigor que no tienen.

**Esta semana — desbloquear el uso.** Ninguna de las dos rompe contratos:

1. Acotar y paginar `duplicate_report` y `structural_review_queue`, con agregado
   por defecto. Sin esto no hay uso conversacional con ningún modelo.
2. `df-mcp` sin bloquear: arrancar-y-sondear.

**El cambio de fondo.** ADR del reencuadre de perfil (§4). Es la decisión más
importante pendiente y cambia el alcance de M2.3.

**Solo lo puede hacer el autor.** La muestra estratificada auditada (§3). Requiere
conocimiento de dominio, que es exactamente lo que faltó en los diez días.

**Después.** Registro de vitalidad → worker desacoplado. Migraciones 0021 y 0022
(prematuras: el reencuadre cambia qué son los parámetros). M2.3 tal como está
escrito hoy — no construir lo que se va a redefinir.

**Sigue abierta desde antes.** La precedencia recomendación vs. prueba de
ADR-0045: bloquea 108,5 GB y `df-rules` heredará el mismo techo.
