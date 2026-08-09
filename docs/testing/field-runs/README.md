# Ejecuciones sobre datos reales

Qué pasó cuando DataForge se apuntó a un archivo de verdad. Un fichero por
ejecución, `AAAA-MM-DD-descripcion.md`.

**Una prueba sin fichero aquí es una prueba que el resto del proyecto no puede
ver.** Ya ha pasado: las cifras que sostienen ROADMAP-2.0 —158.219 archivos,
239,7 GB redundantes, 5.334 items de revisión— salieron de una ejecución real
en la oficina que nadie registró. Los números se citan; la ejecución que los
produjo no es reproducible ni comprobable por nadie más.

Es la misma disciplina que [`m0.8-scale-1m.md`](../m0.8-scale-1m.md) ya aplica
al run de un millón de archivos, extendida a las pruebas de campo.

## Qué registrar

Lo que haga falta para que otro entienda el resultado sin haber estado
delante:

- **Corpus**: qué era, cuántos archivos, cuántos bytes, de dónde venía.
  Sin datos personales ni rutas de cliente — el perfil y la forma bastan.
- **Binario**: versión y commit. `dataforge --version` y el SHA que se usó.
- **Máquina**: sistema, disco de origen y de destino (local, NAS, USB), y el
  filesystem que DataForge clasificó.
- **Qué se ejecutó**: los comandos, en orden, con sus opciones.
- **Qué salió**: los contadores de cada etapa y el veredicto de verificación.
  Pegar la salida, no resumirla de memoria.
- **Qué salió mal**, si algo salió mal. Es la parte con más valor.
- **Cuánto tardó cada etapa**, aunque sea aproximado. Es lo único que permite
  decir después si un cambio mejoró algo.

## Reglas

**Escribir de memoria vale, siempre que lo diga.** Un registro que empieza con
«reconstruido de memoria, sin salida guardada» es infinitamente mejor que
ningún registro. Lo que no vale es escribir de memoria y presentarlo como
medido.

**No inventar precisión.** Si no se cronometró, se dice «unas dos horas», no
«7.412 s».

**Los datos crudos, dentro.** Si hay JSON, log o export, va junto al `.md`, por
la misma razón que el benchmark de M1.0.1 versiona los suyos en
`docs/performance/data/`: una cifra cuya evidencia no está en el repositorio es
una cifra que nadie puede comprobar.

**Nada del cliente.** Ni rutas reales, ni nombres, ni contenido. La forma del
archivo y los contadores bastan para todo lo que este directorio sirve.

## Plantilla

```markdown
# <fecha> — <qué se probó>

**Binario:** dataforge X.Y.Z (`<sha>`) · **Equipo:** <so, disco origen → destino>
**Perfil:** generic | legal · **Fuente de los datos:** medido | de memoria

## Corpus
<forma, cuántos archivos, cuántos bytes, cómo estaba de desordenado>

## Qué se ejecutó
```
<comandos, en orden>
```

## Resultado
<contadores por etapa, veredicto de verificación, tiempos>

## Qué salió mal
<o «nada», que también es un dato>

## Qué aprendimos
<lo que cambiaría el producto o el roadmap>
```
