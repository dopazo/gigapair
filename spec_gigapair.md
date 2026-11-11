# Spec: Gigapair, transferencia P2P de archivos grandes (Rust + GPUI)

Nov 11, 2026 · @Diego Opazo

## Contexto y objetivo

Construiremos una app de escritorio en Rust, con UI en GPUI, para enviar archivos grandes (de 50 a 200 GB o más, de cualquier tipo) directo entre dos equipos, de forma rápida, cifrada, íntegra y reanudable.

El caso de uso inicial es enviar grabaciones de charlas entre dos personas para su edición. Hoy no hay una forma cómoda de mover archivos de ese tamaño: los servicios en la nube como Google Drive se quedan sin espacio, WeTransfer tiene límites y las herramientas CLI (croc, sendme) funcionan pero no son cómodas para el día a día.

Objetivos:

- Transferir archivos y carpetas 1 a 1, sin pasar por almacenamiento en la nube.
- Garantizar integridad (verificación por hash) y cifrado de extremo a extremo (end-to-end encryption).
- Tolerar cortes y reanudar sin perder lo ya transferido.
- Ser simple: emparejar una vez, después arrastrar y soltar.
- Ser rápida y liviana: binario nativo, bajo uso de RAM, arranque inmediato.

**Posicionamiento.** El foco es mover archivos muy grandes entre equipos que se comunican seguido. Dos ideas guían el producto:

- **Archivos grandes primero**: transferencias de cientos de GB que sobreviven cortes, no ocupan el doble de espacio y no se interrumpen porque el equipo se suspendió.
- **Contactos frecuentes**: se empareja una vez y después se envía arrastrando y soltando, sin códigos por cada envío; incluso a un equipo servidor que recibe solo, por CLI o servicio.

Lema: *"Move huge files between the devices you trust. Pair once, send anytime."*

## Alcance y no-objetivos

La versión 1 cubre transferencias 1 a 1 entre contactos emparejados, en Windows, macOS y Linux.

Dentro del alcance:

- Envío y recepción de archivos individuales, múltiples archivos y carpetas.
- Emparejamiento persistente entre dos equipos.
- Reanudación tras cortes de red, cierre de la app o reinicio del equipo.
- Evitar la suspensión del equipo solo mientras hay transferencias activas.
- Instaladores para los tres sistemas operativos.

Fuera del alcance (no-objetivos) en v1:

- Transferencias de uno a muchos o enjambre (swarm) tipo torrent.
- Almacenamiento en la nube o nodo intermedio que guarde archivos cuando el receptor está apagado.
- Sincronización bidireccional de carpetas (estilo Syncthing).
- Compresión o transcodificación de video (se hace antes, con HandBrake u otra herramienta).
- Apps móviles.

## Requisitos funcionales

Las prioridades usan MoSCoW: Must para v1, Should si alcanza, Could para después.

| ID | Requisito | Prioridad |
| --- | --- | --- |
| RF-01 | Emparejar dos equipos: la app muestra un código de emparejamiento como texto con botón "Copiar" y como QR (para copiarlo con el celular y enviarlo por WhatsApp, correo, etc.); el otro lo pega en "Agregar contacto" | Must |
| RF-02 | Enviar archivos, varios archivos o carpetas a un contacto, con arrastrar y soltar (drag & drop) o selector de archivos | Must |
| RF-03 | Cola de transferencias con estado por ítem: en espera, transfiriendo, pausado, verificando, completo, error | Must |
| RF-04 | Progreso con porcentaje, velocidad, tiempo estimado (ETA) y tipo de conexión (directa o relay) | Must |
| RF-05 | Reanudación automática al volver la conexión, sin retransmitir los bloques ya verificados | Must |
| RF-06 | Verificación de integridad BLAKE3 por bloque durante la transferencia y del archivo completo al final | Must |
| RF-07 | Mantener el equipo despierto solo mientras haya transferencias activas, con opción de desactivarlo en ajustes | Must |
| RF-08 | Solo se reciben envíos de contactos emparejados y con la app o el servicio en ejecución. Por defecto cada envío requiere que el receptor lo acepte; un switch por contacto, "Aceptar automáticamente", permite recibir sin confirmación (caso equipo servidor) | Must |
| RF-09 | Verificar espacio libre en el destino antes de aceptar y rechazar con aviso si no alcanza | Must |
| RF-10 | CLI con las mismas funciones que la UI: emparejar, listar contactos, enviar, aceptar, estado | Must |
| RF-11 | Modo servicio (daemon) instalable como servicio del sistema, para equipos receptores sin interfaz | Must |
| RF-12 | Pausar, reanudar y cancelar transferencias manualmente | Should |
| RF-13 | Notificación del sistema al recibir una solicitud de envío y al completar o fallar | Should |
| RF-14 | Ícono en la bandeja del sistema (tray) para seguir funcionando con la ventana cerrada | Should |
| RF-15 | Historial de transferencias enviadas y recibidas | Should |
| RF-16 | Envío puntual por ticket a alguien no emparejado; el receptor siempre confirma | Could |
| RF-17 | Limitar ancho de banda de subida | Could |
| RF-18 | Relay propio configurable en lugar de los relays públicos | Could |

## Requisitos no funcionales

| ID | Área | Requisito |
| --- | --- | --- |
| RNF-01 | Rendimiento | Saturar el enlace disponible en conexión directa; la app no debe ser el cuello de botella |
| RNF-02 | Rendimiento | Archivos de 200 GB o más sin cargarlos en memoria (lectura y escritura en streaming) |
| RNF-03 | Recursos | Arranque en menos de 1 s y uso de RAM en reposo bajo 100 MB (meta, a validar) |
| RNF-04 | Seguridad | Cifrado de extremo a extremo con QUIC/TLS 1.3; identidad de cada equipo por clave pública Ed25519 |
| RNF-05 | Seguridad | La clave privada del nodo se guarda en el almacén de credenciales del sistema (keyring); en modo servidor sin keyring, en archivo con permisos 0600 |
| RNF-06 | Integridad | Ningún archivo se marca como completo sin coincidir el hash BLAKE3 raíz |
| RNF-07 | Resiliencia | Estado de transferencias persistido en disco; sobrevive a cierre de la app y reinicio del equipo |
| RNF-08 | Conectividad | Funcionar detrás de NAT y CGNAT, con hole punching y relay como respaldo |
| RNF-09 | Plataformas | Windows 10/11, macOS (Apple Silicon e Intel) y Linux con Wayland (GNOME, KDE, COSMIC en Pop!\_OS 24.04) y X11 |
| RNF-10 | Distribución | Releases automáticos en GitHub Actions con checksums SHA-256 y atestación de build (build provenance); firma de binarios según la sección de repositorio |
| RNF-11 | Mantenibilidad | Núcleo de transferencia independiente de la UI, con versión de GPUI fijada |
| RNF-12 | Espacio en disco | Recibir un archivo no debe ocupar el doble de su tamaño (sin copia final de 200 GB) |

Idioma: la UI, la CLI, los mensajes de error, el README y la documentación del repositorio van en inglés. Los textos de interfaz que aparecen en este spec en español son descriptivos; los definitivos se escriben en inglés, dejando los strings centralizados para traducir más adelante si hace falta.

Compatibilidad, actualizaciones y privacidad:

- **Versión de protocolo**: cada conexión entre equipos y entre cliente y servicio intercambia una versión; si no son compatibles, se muestra un aviso claro en vez de fallar a mitad de una transferencia.
- **Actualizaciones**: la app avisa cuando hay un release nuevo en GitHub; sin actualización automática en v1.
- **Sin telemetría**: nada sale del equipo salvo la conexión con el contacto y, si hace falta, el relay. Los logs quedan locales, con opción de exportarlos para reportar errores.

## Arquitectura y stack

Cada equipo corre un servicio en segundo plano (gigapaird) que es dueño del nodo iroh, la cola y el estado. La UI en GPUI y la CLI son clientes delgados que hablan con ese servicio por IPC local (socket Unix en macOS y Linux, named pipe en Windows). Así, cerrar la ventana no corta transferencias y un equipo servidor puede recibir sin interfaz.

&#91;embedded content: arquitectura · 2 equipos, 3 capas, relay de respaldo\]

La conexión intenta ir directa por QUIC con hole punching; si el NAT lo impide, el tráfico pasa por un relay, sin que el relay pueda leer el contenido.

Por defecto se usan los relays públicos de n0, sin configuración. Como un relay es un intermediario, la app siempre deja explícito por dónde viaja cada transferencia:

- **Indicador por transferencia**: "Directa" o "Vía relay", con el nombre del relay (por ejemplo, el de n0 o uno propio), en la UI y en `gigapair status`.
- **Aviso la primera vez** que se usa un relay: explica que el relay solo reenvía datos cifrados de extremo a extremo y no puede leer los archivos, pero sí ve las direcciones IP de ambos equipos, el momento y el volumen transferido.
- **Ajustes**: muestran el relay configurado; permiten cambiarlo por uno propio o activar "Solo conexión directa", que prefiere fallar antes que usar un intermediario.
- **Historial**: registra si cada transferencia fue directa o por relay.

| Componente | Tecnología | Rol |
| --- | --- | --- |
| UI | GPUI (+ gpui-component), versión fijada | Ventana, drag & drop, progreso, ajustes |
| CLI | clap | Comandos para uso por terminal y scripts |
| Servicio | gigapaird sobre Tokio | Dueño del nodo: cola, contactos, estado, reintentos, eventos a los clientes |
| IPC local | interprocess (socket Unix / named pipe) | Comunicación UI/CLI ↔ servicio |
| Red | iroh | Conexión QUIC cifrada, identidad por NodeId, hole punching, relay |
| Transferencia | iroh-blobs | Blobs y colecciones con verificación BLAKE3 y reanudación |
| Suspensión | keepawake | Guard activo solo con transferencias en curso (idle on, display off) |
| Persistencia | SQLite con rusqlite (opción bundled, compilado dentro del binario) | Contactos, cola e historial |
| Rutas | directories | Rutas estándar de configuración y datos en cada sistema |
| Claves | keyring | Clave privada del nodo en el almacén del sistema |
| Notificaciones y tray | notify-rust, tray-icon | Avisos e ícono en bandeja |
| Empaquetado | dist (cargo-dist) + GitHub Actions | Instaladores, checksums y releases |

Estructura del repositorio (workspace de Cargo):

- `crates/gigapair-core`: lógica de transferencia y protocolo, sin dependencias de UI.
- `crates/gigapaird`: el servicio; incluye instalación como servicio del sistema.
- `crates/gigapair-cli`: CLI, cliente del servicio.
- `crates/gigapair-app`: app GPUI, cliente del servicio; si el servicio no corre, lo inicia.

## Flujos principales

**Emparejamiento (una vez)**

1. A elige "Agregar contacto"; la app muestra un código de emparejamiento como texto, con botón "Copiar", y como QR.
2. A envía el código por el medio que quiera: lo copia y lo pega en un chat o correo, o lo escanea con el celular y lo reenvía por WhatsApp.
3. B pega el código en "Agregar contacto"; ambos equipos se conectan y muestran una huella de verificación (fingerprint) de 4 palabras.
4. Ambos confirman que la huella coincide y le ponen nombre al contacto.
5. Cada lado guarda el NodeId del otro, con "Aceptar automáticamente" desactivado por defecto.

El código de emparejamiento es de un solo uso y expira a los 10 minutos. Eliminar un contacto revoca su acceso de inmediato, en ambos sentidos.

**Envío**

1. A arrastra archivos o carpetas sobre el contacto B.
2. El servicio de A calcula el hash BLAKE3 en streaming y crea una colección (collection) en iroh-blobs; la UI muestra "preparando".
3. A envía a B una solicitud con el hash y los metadatos (nombres, tamaños).
4. B verifica que haya espacio libre. Si el contacto tiene "Aceptar automáticamente", acepta; si no, notifica y espera que el usuario acepte o rechace. Un rechazo se informa a A.
5. B descarga en una carpeta oculta `.gigapair-partial/` dentro de la carpeta destino (mismo disco), verificando cada bloque al llegar. keepawake queda activo en ambos equipos.
6. Al terminar, B verifica el hash raíz y mueve el archivo a su nombre final con un renombrado (rename) atómico: no se copia, así que ocupa el espacio una sola vez.
7. B confirma a A, se libera keepawake si no quedan transferencias activas y se envía la notificación.

**Reanudación**

1. Se corta la red, se cierra la app o se apaga un equipo.
2. Al volver, la app lee el estado persistido y reintenta conectar con backoff exponencial.
3. iroh-blobs detecta los bloques ya verificados y pide solo los faltantes.
4. La transferencia continúa desde donde quedó, sin intervención del usuario.

Mientras se transfiere, el archivo no aparece con su nombre final en la carpeta destino; así nadie abre o edita un archivo incompleto. En la etapa 1 hay que validar que iroh-blobs permita guardar los datos en esa carpeta y exportar sin copia. Para archivos de 200 GB, el hash inicial lleva tiempo (BLAKE3 suele ir a varios GB/s, limitado por el disco), así que esa etapa se muestra como progreso propio.

**Casos borde de archivos**

- Si ya existe un archivo con el mismo nombre en el destino, se renombra a `nombre (1).ext`; nunca se sobrescribe.
- Se conservan la estructura de carpetas (incluidas las vacías) y la fecha de modificación.
- Los enlaces simbólicos (symlinks) no se siguen; se omiten con aviso.
- Nombres inválidos en el sistema receptor (por ejemplo `:` de Linux a Windows) se ajustan, y se soportan rutas largas en Windows.

## UI

Una sola ventana con barra lateral de contactos y un panel de transferencias; nada de asistentes de varios pasos.

| Vista | Contenido |
| --- | --- |
| Barra lateral | Contactos con estado (en línea, desconectado), botón "Agregar contacto" |
| Panel principal | Zona de arrastrar y soltar del contacto seleccionado y cola de transferencias activas |
| Fila de transferencia | Nombre, tamaño, barra de progreso, velocidad, ETA, conexión directa o relay, botones pausar y cancelar |
| Historial | Lista de transferencias completas o fallidas, con abrir carpeta y reintentar |
| Ajustes | Carpeta de descarga, evitar suspensión (on/off), inicio con el sistema, relay propio, límite de subida |
| Bandeja (tray) | Progreso global, abrir ventana, salir |

Pautas:

- Tema claro y oscuro siguiendo el sistema.
- Indicador visible "Evitando suspensión" mientras keepawake esté activo.
- Aviso no bloqueante si la conexión es por relay (más lenta).
- Componentes base desde gpui-component, si su estado lo permite; si no, componentes propios mínimos.

## CLI y modo servicio

La CLI ofrece lo mismo que la UI y además instala el servicio, que es lo que permite tener un equipo servidor que solo recibe. El binario es `gigapair`, con `gpair` como alias corto opcional.

| Comando | Qué hace |
| --- | --- |
| `gigapair pair` | Muestra el código de emparejamiento como texto y como QR en la terminal |
| `gigapair pair <código>` | Agrega el contacto a partir del código del otro equipo |
| `gigapair contacts` | Lista contactos; subcomandos `rename`, `remove` y `trust <contacto> --auto-accept on\|off` |
| `gigapair send <contacto> <rutas...>` | Envía archivos o carpetas y muestra el progreso |
| `gigapair requests` / `accept <id>` / `reject <id>` | Lista solicitudes pendientes y las acepta o rechaza |
| `gigapair status [--watch]` | Transferencias activas, velocidad y tipo de conexión |
| `gigapair history` | Historial de envíos y recepciones |
| `gigapair config get\|set <clave> <valor>` | Lee o cambia la configuración |
| `gigapair service install\|uninstall\|start\|stop\|status` | Gestiona el servicio del sistema (`gigapaird`) |

El servicio se instala con el mecanismo nativo de cada sistema:

- **Linux**: unidad de usuario de systemd; en un servidor sin sesión, `loginctl enable-linger` para que corra sin login, o unidad de sistema con usuario dedicado.
- **macOS**: LaunchAgent en `~/Library/LaunchAgents`.
- **Windows**: tarea al iniciar sesión para uso normal, o Windows Service para un equipo servidor.

Configuración de ejemplo para un equipo servidor:

```toml
download_dir = "/srv/gigapair/incoming"
keep_awake = true

[[contacts]]
name = "workstation"
node_id = "<node id>"
auto_accept = true
```

## Datos locales y rutas

La app usa las rutas estándar de cada sistema (crate `directories`); nada se guarda junto al ejecutable.

| Dato | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Configuración (`config.toml`) | `%APPDATA%\Gigapair\config\` | `~/Library/Application Support/dev.gigapair.Gigapair/` | `~/.config/gigapair/` |
| Base de datos: contactos, cola, historial | `%LOCALAPPDATA%\Gigapair\data\` | `~/Library/Application Support/dev.gigapair.Gigapair/` | `~/.local/share/gigapair/` |
| Logs | `%LOCALAPPDATA%\Gigapair\logs\` | `~/Library/Logs/Gigapair/` | `~/.local/state/gigapair/` |
| Clave privada del nodo | Administrador de credenciales | Llavero (Keychain) | Secret Service (GNOME Keyring, KWallet) |
| Descargas por defecto | `%USERPROFILE%\Downloads\Gigapair\` | `~/Downloads/Gigapair/` | `~/Downloads/Gigapair/` |
| Datos parciales en tránsito | `.gigapair-partial\` dentro de la carpeta destino | igual | igual |

En un servidor Linux con servicio de sistema: configuración en `/etc/gigapair/` y datos en `/var/lib/gigapair/`, con la clave en un archivo de permisos 0600.

## Repositorio, licencia y releases

El código vive en el repositorio público [dopazo/gigapair](https://github.com/dopazo/gigapair), con licencia MIT y releases automáticos con GitHub Actions.

- **Registros**: antes de publicar, reservar el crate `gigapair` en crates.io y, opcionalmente, el dominio gigapair.dev.
- **Licencia**: MIT. Las dependencias deben ser compatibles con MIT (por ejemplo MIT, Apache-2.0 o BSD); se evitan las copyleft (GPL, AGPL).
- **Releases**: `dist` (cargo-dist) genera instaladores y el GitHub Release al crear un tag; `release-plz` puede automatizar versiones y changelog.
- **Integridad de descargas**: checksums SHA-256 y atestaciones de build de GitHub, ambos gratis.
- **Canales**: GitHub Releases, winget o Scoop en Windows, tap de Homebrew en macOS, `.deb` y AppImage en Linux.

Firma de binarios:

| Sistema | Opción | Costo | Notas |
| --- | --- | --- | --- |
| Windows | [SignPath Foundation](https://signpath.org/) | Gratis para open source | Exige licencia OSI, build automatizado desde el repo público y aprobación; el certificado sale a nombre de la fundación. [Proyectos pequeños pueden ser rechazados](https://github.com/FrankRay78/NetPace/wiki/Distribution-and-signing) |
| Windows | Certum Open Source Code Signing | Desde €49/año | Certificado a nombre del desarrollador individual |
| Windows | Azure Artifact Signing | US$9.99/mes | [Para individuos, solo EE.UU. y Canadá](https://learn.microsoft.com/en-us/answers/questions/5810735/cant-create-a-new-trusted-signing-individual-ident): no aplica desde Chile |
| macOS | Apple Developer Program | US$99/año | [Sin exención para individuos](https://flaviocopes.com/apple-developer-free-vs-paid/); notarizar no tiene costo adicional |
| Linux | Checksums y atestaciones | Gratis | No requiere firma de código |

Decisión: v1 se publica sin firma, con checksums y atestaciones de build. El README explica cómo pasar la advertencia en cada sistema (Windows: "Más información" → "Ejecutar de todas formas"; macOS: Ajustes → Privacidad y seguridad → "Abrir de todas formas") y ofrece `cargo install`, Homebrew y Scoop como alternativas con menos fricción. La firma se revisa según adopción: SignPath en Windows cuando el proyecto tenga usuarios, y Apple Developer si los usuarios de macOS lo justifican.

## Riesgos

| Riesgo | Impacto | Mitigación |
| --- | --- | --- |
| GPUI pre-1.0 con cambios incompatibles | Actualizaciones rompen la UI | Fijar versión o commit; UI como cliente separado del servicio |
| Poca documentación de GPUI | Desarrollo más lento | Prototipo de UI temprano; usar ejemplos de Zed y gpui-component |
| COSMIC es un compositor Wayland reciente | Detalles de tray, notificaciones o drag & drop pueden fallar | Probar en Pop!\_OS 24.04 desde la etapa 2; la CLI siempre funciona |
| iroh-blobs no permite exportar sin copiar | Recibir 200 GB ocuparía 400 GB | Validar en etapa 1; si no se puede, escribir los archivos directamente con un store propio |
| CGNAT o redes corporativas impiden conexión directa | Transferencias lentas por relay | Mostrar tipo de conexión; opción de relay propio en un VPS |
| Ambos equipos deben tener el servicio corriendo | Transferencias largas se interrumpen | Servicio en segundo plano; reanudación automática; keepawake |
| Suspensión explícita restringida por el SO (Modern Standby, tapa cerrada) | keepawake no basta en algunos equipos | Avisar al usuario; recomendar conectar a la corriente |
| Binarios sin firma | Advertencias de SmartScreen y Gatekeeper | Checksums y atestaciones; SignPath en Windows; Apple Developer si se justifica |
| Cambios de API en iroh / iroh-blobs | Mantención recurrente | Fijar versiones; tests de integración de transferencia |

## Plan por etapas y criterios de aceptación

Cinco etapas, cada una termina en algo usable.

| Etapa | Entregable | Criterio de aceptación |
| --- | --- | --- |
| 0. Validación | Prueba de sendme con un archivo real de 50 GB entre dos equipos | Transferencia completa y reanudada tras un corte forzado |
| 1. Núcleo + CLI | gigapair-core, servicio gigapaird y CLI con pair, send, accept, status y reanudación | 50 GB entre Windows y macOS por red distinta, con 2 cortes, hash idéntico |
| 2. UI mínima | Ventana GPUI con contactos, drag & drop y progreso | Envío completo sin usar terminal |
| 3. Robustez | keepawake, persistencia, cola, historial, notificaciones, tray | Reinicio del equipo a mitad de transferencia y reanudación automática |
| 4. Distribución | Releases con dist en GitHub Actions (MSI, DMG, AppImage, .deb), checksums, atestaciones e instalación del servicio | Instalación limpia en los 3 sistemas y un equipo servidor recibiendo sin interfaz |

Definición de terminado para v1:

- [ ] Transferir 200 GB entre dos equipos en redes distintas sin intervención manual.
- [ ] Integridad verificada por BLAKE3 en el 100% de los casos.
- [ ] Ningún equipo se suspende durante una transferencia activa conectado a la corriente.
- [ ] Funciona en Windows, macOS y Linux.

## Estrategia de pruebas

- **Unitarias** en gigapair-core: cola, estado persistido, manejo de nombres y rutas.
- **Integración**: dos nodos en el mismo proceso que transfieren archivos generados de varios GB y verifican el hash.
- **Fallas simuladas**: cortar la conexión, matar el servicio y reiniciarlo a mitad de una transferencia; debe reanudar sin datos corruptos.
- **CI en GitHub Actions** con matriz Windows, macOS y Linux en cada pull request.
- **Prueba manual por release**: 50 GB entre dos redes reales, una de ellas detrás de CGNAT.
