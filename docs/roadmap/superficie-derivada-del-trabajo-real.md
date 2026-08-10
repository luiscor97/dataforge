# La superficie que el trabajo real necesitó

**Fecha:** 2026-08-10 · **Estado:** propuesta, para discutir antes de implementar

Qué herramientas hacen falta para que un agente —de cualquier proveedor—
ejecute la tarea completa por MCP. No deducido de principios: **derivado de lo
que el trabajo original hizo de verdad**, contrastado contra las 25
herramientas que `df-tools` expone hoy.

La evidencia es
[`2026-06-29-asesoria-juridica-original.md`](../testing/field-runs/2026-06-29-asesoria-juridica-original.md):
un mes de trabajo, 47.982 decisiones etiquetadas, 443,4 GB → 150,57 GB.

## 1. El bucle que funcionó

Reconstruido de la transcripción, el trabajo original tuvo esta forma:

```
inventariar → hashear
  → detectar estructura rota (árboles injertados, nombres colisionando)
  → clasificar cada archivo en un destino, con un motivo
  → planificar → copiar verificando → auditar
  → informe con manifiesto
```

Y se repitió: cada vuelta descubría un caso nuevo (correos que iban dentro del
asunto, vídeos formativos que parecían ocio) y **volvía a clasificar**. No fue
una pasada, fue un bucle de refinamiento con el humano corrigiendo el criterio.

## 2. Qué de eso ya se puede hacer por MCP

| Paso del trabajo real | Herramienta | |
| --- | --- | --- |
| Inventariar sin tocar | `scan_project` | ✅ |
| Identidad por contenido | `hash_project` | ✅ |
| Duplicados exactos | `duplicate_report` | ✅ |
| Árboles clonados y embebidos | `tree_clone_report`, `tree_relation_report` | ✅ |
| Fronteras protegidas | `context_report` | ✅ |
| Cola de dudas, agrupada por clase | `structural_review_classes` | ✅ |
| Contestar por clase, atómico | `decide_structural_review_batch` | ✅ |
| Ver la salida antes de firmar | `plan_destination_tree` | ✅ |
| Copiar y verificar | `execute_plan`, `verify_project_output` | ✅ (cerradas hasta el gate) |

**La mitad del trabajo ya es conducible.** Lo que falta se agrupa en dos
bloques muy distintos, y conviene no confundirlos.

## 3. Lo que rompe el bucle no es la inteligencia — es la mecánica

Esta es la parte que no esperaba encontrar. Un agente con las 25 herramientas
de hoy **no puede terminar la tarea**, y no por falta de criterio:

### 3.1 Las llamadas largas bloquean sin decir nada

`hash_project` sobre 443 GB tarda horas y no devuelve nada hasta terminar. Un
agente por stdio se queda colgado sin señal, sin poder informar, sin poder
decidir si algo va mal. La transcripción original está **llena** de «sigue
vivo», «va por 30/75», «espero al marcador 10/75»: el operador humano
necesitaba ese latido y lo obtenía leyendo la consola.

Y aquí está el matiz que importa: **los contadores ya existen**.
`inventory_summary` (`crates/df-db/src/inventory.rs`) publica `hash_done` /
`hash_pending` y `project_status` ya los expone. El dato está.

Lo que no existe es la forma de leerlo *durante* el trabajo: la llamada
bloqueante ocupa la única sesión stdio, así que el agente no puede
preguntarse a sí mismo cómo va. No falta telemetría; falta **desacoplar
arrancar de esperar**.

- **`job_start(stage)`** → devuelve un id y vuelve enseguida.
- **`job_status(id)`** → fase y los contadores que ya se escriben hoy.

Sin esto no hay bucle: hay una llamada que no vuelve.

### 3.2 Los informes no caben — **resuelto el 2026-08-10**

`duplicate_report` sobre este corpus devolvía 28.537 conjuntos.
`structural_review_queue`, 5.334 elementos. Ninguna ventana de contexto los
aguanta, y un agente que pide un informe y recibe decenas de MB de JSON ha
gastado su sesión sin aprender nada.

Los seis informes con detalle —duplicados, clones de árbol, relaciones,
contexto, anomalías y cola de revisión— devuelven ahora una **ventana**:
`limit`/`offset`, 50 por defecto, techo de 1.000. Tres propiedades, cada una
con test:

- **Los totales nunca se acotan.** Salió gratis porque los informes ya
  calculaban los escalares aparte del vector: `redundant_bytes` significa lo
  mismo con un conjunto que con mil.
- **La truncación siempre es visible**, en `pages.<colección>.has_more`. Una
  truncación que el llamante no puede detectar no es paginación: es una
  respuesta incorrecta con aspecto de correcta.
- **El techo no se negocia.** Se puede pedir menos y nunca más, porque un
  límite que el llamante puede subir no es un límite. El clamp no es silencioso:
  `returned` y `has_more` siguen diciendo la verdad.

La lista de colecciones tiene **una sola definición**, que leen tanto el
dispatch como el esquema MCP — si no, el esquema podría anunciar una ventana
que el dispatch no aplica. Superficie `dataforge.tool-surface/0.3.0`, con
`frozen_contracts` actualizado en el mismo commit (ADR-0037 §2).

### 3.3 No hay forma de saber si un trabajo sigue vivo

El proyecto de la prueba de la v1.0 está ahora mismo en `EXECUTING` porque una
ejecución murió sin pasar por su ruta de pausa. Un agente que se reconecta no
puede distinguir «hay otro proceso copiando» de «murió hace tres días».

Buscado en todo `crates/`: **ni PID, ni host, ni latido, ni marca de
actividad.** No es que esté a medias, es que no hay nada. Es lo que el traspaso
llama PR 2, y es **precondición de la autonomía**, no un adorno: sin ella,
reanudar es apostar.

## 4. Lo que sí es inteligencia, y dónde está el hueco real

El trabajo original produjo, por archivo, una **categoría** y un **motivo**:

| Categoría | Archivos | Motivo dominante |
| --- | ---: | --- |
| `excluido_no_juridico` | 19.413 | «sin señales suficientes» |
| `asesoria_main` | 12.426 | «raíz jurídica reconocida» |
| `correos` | 7.873 | «correo o contenedor» |
| `revision_origen_mixto` | 5.576 | «vocabulario jurídico fuera de raíz» |
| `periciales` | 2.331 | «pericial/fotos/asunto caligráfico» |

**DataForge no tiene ese verbo.** Sabe decidir sobre *items de revisión*
estructurales, pero no puede decir «este archivo va a esta raíz por este
motivo». Es el hueco de M2.3, y es el único que necesita clasificación.

La forma correcta, según el reencuadre pendiente, **no** es que el modelo
juzgue 158.219 archivos:

- **`propose_profile(evidence)`** → el agente propone **un perfil**: raíces
  declaradas, marcadores, reglas. Pequeño, auditable, reutilizable.
- **`validate_profile(profile)`** → el motor lo valida y lo rechaza
  *fail-closed* si no cuadra. Sin escribir nada.
- **`apply_profile(profile)`** → determinista, reproducible, sobre todo el
  corpus, sellando digest y versión.

Un perfil de veinte reglas que un humano puede leer, frente a 158.219
juicios que nadie puede auditar. **Y ya sabemos que funciona**: el trabajo
original acabó exactamente así, con marcadores y raíces, no con un veredicto
por archivo.

## 5. Dos herramientas que la evidencia pide y nadie había propuesto

**`grafted_tree_report`.** `tree_relation_report` da relaciones, pero el
trabajo original necesitó algo más fino: para cada archivo dentro de un
injerto, **su ruta canónica probable** y en cuál de estos cuatro casos cae:

| | | |
| --- | ---: | --- |
| en su ruta canónica, mismo hash | 130.165 | 96,1 % → automático |
| el contenido existe fuera del injerto | 3.977 | 2,9 % → automático |
| **contenido único dentro del injerto** | **817** | 0,6 % → revisión |
| **misma ruta canónica, contenido distinto** | **419** | 0,3 % → revisión |

**99,1 / 0,9.** Ese es el umbral de auto-colocación para árboles injertados, y
no es una suposición: es el reparto medido sobre 135.378 archivos.

**`name_collision_report`.** El caso que ninguna regla de contenido detecta:
106 nombres con contenido distinto entre asuntos, el peor
`00000001.JPG` con **19 hashes en 6 periciales**. Deduplicar por nombre ahí no
es desorden, es destruir prueba. El motor ya se niega a deduplicar por nombre;
lo que falta es **poder demostrar por qué**, que es lo que convence a un
asesor.

## 6. Y una que no es del motor pero decide la entrega

El criterio de aceptación real no fue técnico. Fue, literal: *«que el asesor no
tenga desconfianza porque se haya podido perder material alguno»*. Por eso el
entregable acabó siendo informe + PDF + CSV de trazabilidad + manifiesto
SHA-256 dentro de la propia carpeta.

- **`export_delivery_package()`** → manifiesto de la salida, mapa
  origen→destino con motivo, y el resumen de garantías. El motor ya tiene todo
  el dato; lo que no tiene es la forma de entregarlo.

Un resultado correcto que no se puede demostrar no sirve.

## 7. Orden propuesto

Primero lo mecánico, porque sin ello no hay bucle que optimizar:

1. ~~Agregados por defecto en los informes.~~ **Hecho el 2026-08-10** (§3.2).
   **`job_start` / `job_status`**, que es lo que queda para convertir «una
   llamada que no vuelve» en «un trabajo que se supervisa».
2. **Vitalidad** (PID, host, latido). Precondición de reanudar sin apostar.
3. **`grafted_tree_report`** y **`name_collision_report`**. Son lectura sobre
   evidencia que ya existe en la base; baratos y desbloquean el 99,1 %.
4. **El trío de perfil** (`propose` / `validate` / `apply`), que necesita la
   ADR del reencuadre de M2.3 antes de escribirse.
5. **`export_delivery_package`**.

Y solo después, `df-rules` como autoridad del gate: hasta entonces la clase
`commit` sigue cerrada y el humano ocupa la puerta.

## 8. Lo que no hay que añadir

- **Nada que abra el vocabulario.** Ni FS arbitrario, ni SQL crudo, ni shell.
  La frontera de transporte es lo que hace segura toda la superficie; una sola
  herramienta genérica la anula entera.
- **Ningún verbo que borre o mueva en el origen.** El trabajo original tampoco
  lo necesitó: copió y anotó. `D:\Discolocal` sigue intacto un mes después, y
  esa es la razón de que se pudiera repetir.
- **Ninguna herramienta que decida sin dejar procedencia.** Cada decisión del
  trabajo original tiene motivo escrito. Es lo que permitió auditarla hoy, un
  mes más tarde, desde otra máquina.
