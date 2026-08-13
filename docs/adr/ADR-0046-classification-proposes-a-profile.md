# ADR-0046 — La clasificación se propone como perfil, no como veredicto por archivo

**Estado:** Propuesta
**Fecha:** 2026-08-10
**Relacionada con:** RFC-0001 §5.3, §15.2; RFC-0002 (escalera de autonomía,
pasos 1 y 2); ADR-0026 (perfiles declarativos); ADR-0040 (taxonomía de destino
declarada); ADR-0041 (`df-rules`); ADR-0042 (consentimiento por política);
ADR-0043 (superficie de herramientas); ROADMAP-2.0 M2.3

## Contexto

M2.3 es el cuello de botella de la 2.0: los crates de M2.4, M2.5 y M2.6 existen
y esperan a que el motor sepa **qué es** cada archivo, no solo en qué estado
está. La pregunta abierta no es si hace falta clasificación, sino **qué forma
tiene lo que produce la IA**.

La lectura ingenua —y la que el roadmap arrastraba— es que un agente emite un
veredicto por archivo. Sobre el archivo real eso son 158.219 veredictos. Hay
cuatro hechos medidos que desaconsejan esa forma.

### 1. El trabajo original no terminó en veredictos por archivo

La transcripción del trabajo que sirve de referencia describe ocho categorías
con recuentos —`excluido_no_juridico` 19.413, `asesoria_main` 12.426, `correos`
7.873, `revision_origen_mixto` 5.576, `periciales` 2.331, y tres menores— sobre
47.982 decisiones. Leído de lejos parece un juicio por archivo.

Leído de cerca, no lo es: las categorías se derivan de **marcadores y raíces
declaradas** —«raíz jurídica reconocida», «vocabulario jurídico fuera de
raíz»—, no de una lectura individual. Lo que se decidió una vez fue el
criterio; lo que se aplicó 47.982 veces fue una regla.

### 2. Y las decisiones que sí quedaron registradas no las tomó nadie

Abriendo la base del trabajo original (`decisions`, 47.982 filas):

| | |
| --- | --- |
| Grupos decididos | 47.982 |
| **Que revisó un humano** | **0** |
| Que un humano cambió | 0 |

Las tomó todas una fórmula de puntuación (`depth`, `path_length`,
`oldest_mtime`), y el 98,3% con confianza alta. Esas 47.982 filas **no son
juicios semánticos**: son elecciones de ruta canónica dentro de grupos de
duplicados. El corpus no contiene, en ningún sitio, un veredicto humano por
archivo del que aprender ni contra el que calibrar.

### 3. La mayor parte de la colocación no necesita criterio

`grafted_tree_report` (implementado el 2026-08-10) mide, sobre los archivos que
viven dentro de un árbol injertado, cuántos se colocan por evidencia y cuántos
requieren un humano: **97,9% / 2,1%** sobre 87.667 archivos, y 99,1% / 0,9% con
la derivación de la transcripción sobre 135.378. La afirmación robusta es el
orden de magnitud: **alrededor del 98% se coloca sin juicio alguno**, porque su
hash existe fuera del injerto.

Gastar una llamada a un modelo en ese 98% es peor en coste y **peor en
auditoría**: sustituye una demostración por una opinión.

### 4. Un veredicto por archivo no se puede auditar

158.219 juicios no los revisa nadie, ni el día que se emiten ni un año después
cuando alguien pregunta por qué un documento acabó donde acabó. Un perfil de
veinte reglas sí: se lee entero en cinco minutos, se versiona, se difunde y se
vuelve a aplicar sobre otro corpus.

RFC-0001 §5.3 exige que el sistema sea explicable por diseño. Una respuesta que
solo puede explicarse repitiendo la llamada al modelo no lo es.

## Decisión

1. **La IA propone un perfil, nunca una disposición por archivo.** La unidad de
   salida de la clasificación asistida es un **perfil declarativo** —raíces de
   destino, marcadores, reglas y sus pesos— del mismo tipo que ADR-0026 y
   ADR-0040 ya definen. No se añade ninguna herramienta que emita el destino de
   un archivo concreto.

2. **Tres verbos, y la frontera entre ellos es lo que importa.**
   - `propose_profile(evidence)` — clase `build`. Devuelve un perfil candidato
     a partir de evidencia ya calculada. No escribe nada.
   - `validate_profile(profile)` — clase `observe`. El motor lo comprueba
     *fail-closed* y **no escribe nada**: raíces requeridas, ids únicos,
     nombres de carpeta sin separador ni `..`, reglas resolubles. Un perfil que
     no valida se rechaza entero; no se recorta.
   - `apply_profile(profile)` — clase `build`. Aplicación **determinista** de un
     perfil validado sobre el snapshot, sellando el digest y la versión del
     perfil en cada operación enrutada.

3. **La aplicación es determinista y reproducible.** Dado el mismo snapshot y
   el mismo perfil, la salida es idéntica byte a byte. Ninguna llamada a un
   modelo participa en `apply_profile`: el modelo ya hizo su trabajo al
   proponer.

4. **La evidencia gana al perfil cuando la hay.** Cuando
   `grafted_tree_report` clasifica una aparición como colocable por hash, esa
   colocación **no la decide el perfil**: el perfil solo interviene donde la
   evidencia no alcanza. Un perfil no puede mover un archivo cuya ubicación
   canónica está demostrada.

5. **Lo que el perfil no sabe manejar va a `revisar/`**, con su motivo, y el run
   no se detiene (garantía de no-bloqueo de RFC-0002).

6. **El perfil propuesto es un artefacto, no un efecto.** Se guarda, se
   versiona y se puede leer, editar y volver a aplicar sin repetir la
   propuesta. Un humano puede escribirlo a mano sin IA de por medio, y esa es
   la prueba de que la frontera está bien puesta.

## Alternativas consideradas

- **Veredicto por archivo emitido por el modelo.** Descartada por los cuatro
  hechos de arriba: no reproduce lo que hizo el trabajo original, no hay datos
  contra los que calibrarla, desperdicia una demostración en el 98% de los
  casos y no se puede auditar. Es además la alternativa más cara por dos
  órdenes de magnitud.

- **Veredicto por archivo solo para el residuo ambiguo.** Más defendible: el 2%
  son ~1.700 archivos, que sí cabe consultar. Descartada **por ahora** porque
  el residuo tampoco es homogéneo —«contenido único dentro del injerto» y
  «misma ruta canónica, contenido distinto» son preguntas distintas— y porque
  aceptar la forma «veredicto por archivo» para un caso la normaliza para
  todos. Revisable una vez el perfil esté en producción y se mida qué queda.

- **Clasificación puramente determinista, sin IA.** Es lo que hay hoy, y no
  basta: sin marcadores declarados el 63% del volumen acaba en la bolsa de
  revisión. Alguien tiene que **redactar** el perfil, y redactarlo mirando un
  archivo de 443 GB es precisamente donde un modelo ayuda.

- **Que el perfil lo genere el modelo y se aplique sin validación.** Descartada
  sin matices. Un perfil es código declarativo: aplicar uno no validado es
  ejecutar lo que un modelo escribió, sobre datos probatorios, sin que nada lo
  haya mirado.

## Consecuencias

**Positivas.** La superficie de lo que un modelo puede equivocarse pasa de
158.219 decisiones a un documento revisable. El coste de la fase asistida deja
de escalar con el tamaño del archivo, lo que hace realista el presupuesto de
ADR-0042. La reproducibilidad se conserva: `apply_profile` no llama a nadie. Y
el perfil es transferible — el criterio de un despacho se reutiliza en el
siguiente corpus, que es la diferencia entre una herramienta y un servicio.

**Negativas.** Un perfil malo se equivoca **de forma sistemática**, no
aleatoria: donde un veredicto por archivo produce errores dispersos, una regla
mal puesta desplaza una clase entera. Se mitiga con la validación fail-closed,
con el fallback universal a `revisar/` y con la reversibilidad total (el origen
no se toca, la salida se borra). Y obliga a un ciclo más lento: proponer,
revisar, aplicar, mirar el resultado.

**Neutras.** No cambia ninguna garantía del motor ni el orden de seguridad del
planificador. No toca contratos congelados por sí misma: los mueve la
implementación, que subirá el schema de perfil y la expectativa de
`frozen_contracts` en el mismo commit (ADR-0037 §2).

**Revisar si.** Si al aplicar perfiles sobre dos o tres corpus distintos resulta
que cada uno necesita un perfil desde cero y no se reutiliza nada, la ventaja de
auditabilidad se mantiene pero la de transferibilidad desaparece, y habría que
reconsiderar cuánto esfuerzo merece la propuesta asistida frente a escribir el
perfil a mano. Y si el residuo ambiguo resulta ser mucho mayor que el 2% medido
aquí, la segunda alternativa vuelve a la mesa.
