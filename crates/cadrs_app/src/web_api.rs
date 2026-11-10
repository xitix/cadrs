//! Generic byte-oriented API for an embedding page in the browser build.

use std::path::Path;

use bevy::prelude::*;
use js_sys::{ArrayBuffer, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

use crate::{ActiveDocument, AppClock, AppState, DocumentStore, UserProfile};
use cadrs_core::import::{ImportAs, ImportFormat, ImportIds, imported_document};
use cadrs_core::rebuild::PendingJob;
use cadrs_core::{Document, DocumentId, DocumentMeta};

const MAX_BYTES: usize = 512 * 1024 * 1024;
const UPLOADS: &str = "/Uploads";

#[wasm_bindgen(inline_js = "
export function cadrsApiPoll() {
  return globalThis.cadrsHostApi?.poll() ?? null;
}
export function cadrsApiRespond(message) {
  globalThis.cadrsHostApi?.respond(message);
}
")]
extern "C" {
    fn cadrsApiPoll() -> JsValue;
    fn cadrsApiRespond(message: JsValue);
}

pub struct WebApiPlugin;

impl Plugin for WebApiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveApiDocument>()
            .add_systems(Update, (poll_host_requests, finish_step_imports));
    }
}

#[derive(Resource, Default)]
struct ActiveApiDocument(Option<(String, DocumentId)>);

#[derive(Component)]
struct StepImport {
    request_id: String,
    document_key: String,
    file_name: String,
    data: String,
    pending: PendingJob<Result<cadrs_core::import::ImportPlan, String>>,
}

struct Request {
    id: String,
    op: String,
    key: String,
    name: String,
    bytes: Option<Vec<u8>>,
}

fn property(value: &JsValue, key: &str) -> Option<JsValue> {
    Reflect::get(value, &JsValue::from_str(key))
        .ok()
        .filter(|value| !value.is_undefined() && !value.is_null())
}

fn string_property(value: &JsValue, key: &str) -> Option<String> {
    property(value, key)?.as_string()
}

fn request(value: &JsValue) -> Result<Request, String> {
    if string_property(value, "protocol").as_deref() != Some("cadrs.api")
        || property(value, "version").and_then(|v| v.as_f64()) != Some(1.0)
    {
        return Err("Unsupported API protocol or version".into());
    }
    let id = string_property(value, "requestId").ok_or("Missing requestId")?;
    let op = string_property(value, "op").ok_or("Missing operation")?;
    let key = string_property(value, "documentKey").ok_or("Missing documentKey")?;
    if id.is_empty() || id.len() > 128 || key.is_empty() || key.len() > 128 {
        return Err("Invalid requestId or documentKey".into());
    }
    let name = string_property(value, "name").unwrap_or_default();
    if name.len() > 1024 {
        return Err("File name is too long".into());
    }
    let bytes = if let Some(value) = property(value, "bytes") {
        if !matches!(op.as_str(), "open-cadrs" | "import-step") {
            return Err("This operation does not accept bytes".into());
        }
        if !value.is_instance_of::<ArrayBuffer>() {
            return Err("bytes must be an ArrayBuffer".into());
        }
        let array = Uint8Array::new(&value);
        if array.length() as usize > MAX_BYTES {
            return Err("Payload exceeds the 512 MiB limit".into());
        }
        Some(array.to_vec())
    } else {
        None
    };
    Ok(Request {
        id,
        op,
        key,
        name,
        bytes,
    })
}

fn set(object: &Object, key: &str, value: &JsValue) {
    if Reflect::set(object, &JsValue::from_str(key), value).is_err() {
        error!("cannot build CADRS web API response property {key}");
    }
}

fn respond(
    id: &str,
    op: &str,
    key: &str,
    result: Result<Option<(String, String, Vec<u8>)>, String>,
    document_name: Option<&str>,
) {
    let response = Object::new();
    set(&response, "protocol", &JsValue::from_str("cadrs.api"));
    set(&response, "version", &JsValue::from_f64(1.0));
    set(&response, "requestId", &JsValue::from_str(id));
    set(&response, "op", &JsValue::from_str(op));
    set(&response, "documentKey", &JsValue::from_str(key));
    let result_object = Object::new();
    let result = result.and_then(|file| {
        if file
            .as_ref()
            .is_some_and(|(_, _, bytes)| bytes.len() > MAX_BYTES)
        {
            Err("Export exceeds the 512 MiB limit".into())
        } else {
            Ok(file)
        }
    });
    match result {
        Ok(file) => {
            set(&response, "ok", &JsValue::TRUE);
            if let Some(name) = document_name {
                set(&result_object, "documentName", &JsValue::from_str(name));
            }
            if let Some((name, mime, bytes)) = file {
                set(&result_object, "name", &JsValue::from_str(&name));
                set(&result_object, "mimeType", &JsValue::from_str(&mime));
                set(
                    &result_object,
                    "bytes",
                    &Uint8Array::from(bytes.as_slice()).into(),
                );
            }
            set(&response, "result", result_object.as_ref());
        }
        Err(why) => {
            set(&response, "ok", &JsValue::FALSE);
            set(&response, "error", &JsValue::from_str(&why));
        }
    }
    cadrsApiRespond(response.into());
}

fn poll_host_requests(world: &mut World) {
    for _ in 0..4 {
        let value = cadrsApiPoll();
        if value.is_null() || value.is_undefined() {
            break;
        }
        let parsed = match request(&value) {
            Ok(request) => request,
            Err(error) => {
                let id = string_property(&value, "requestId").unwrap_or_default();
                let op = string_property(&value, "op").unwrap_or_default();
                let key = string_property(&value, "documentKey").unwrap_or_default();
                if !id.is_empty() {
                    respond(&id, &op, &key, Err(error), None);
                } else {
                    warn!("ignoring CADRS API request without requestId");
                }
                continue;
            }
        };
        let import_in_progress = {
            let mut query = world.query_filtered::<Entity, With<StepImport>>();
            query.iter(world).next().is_some()
        };
        if import_in_progress
            && matches!(
                parsed.op.as_str(),
                "create-new" | "open-cadrs" | "import-step" | "save-cadrs" | "export-step"
            )
        {
            respond(
                &parsed.id,
                &parsed.op,
                &parsed.key,
                Err("Another document operation is still in progress".into()),
                None,
            );
            continue;
        }
        handle_request(world, parsed);
    }
}

fn handle_request(world: &mut World, request: Request) {
    let Request {
        id,
        op,
        key,
        name,
        bytes,
    } = request;
    match op.as_str() {
        "create-new" => {
            let name = if name.trim().is_empty() {
                "Untitled document"
            } else {
                name.trim()
            };
            let doc = Document::new(name);
            match store_new_document(world, doc) {
                Ok((doc_name, _)) => {
                    set_active_key(world, key.clone());
                    respond(&id, &op, &key, Ok(None), Some(&doc_name));
                }
                Err(error) => respond(&id, &op, &key, Err(error), None),
            }
        }
        "open-cadrs" => {
            let Some(bytes) = bytes else {
                return respond(&id, &op, &key, Err("Missing .cadrs bytes".into()), None);
            };
            let path = Path::new(UPLOADS).join(format!("{}.cadrs", uuid::Uuid::new_v4()));
            let result = cadrs_fs::create_dir_all(UPLOADS)
                .and_then(|()| cadrs_fs::write(&path, bytes))
                .map_err(|e| e.to_string())
                .and_then(|()| {
                    let store = world.resource::<DocumentStore>().0.clone();
                    let user = world.resource::<UserProfile>().id.clone();
                    let now = world.resource::<AppClock>().now();
                    let result = cadrs_core::native::import(&path, &store, &user, now);
                    let _ = cadrs_fs::remove_file(&path);
                    result
                });
            match result {
                Ok(file) => {
                    let doc_name = file.document.name.clone();
                    open_document(world, file.document, file.meta);
                    set_active_key(world, key.clone());
                    respond(&id, &op, &key, Ok(None), Some(&doc_name));
                }
                Err(error) => respond(&id, &op, &key, Err(error), None),
            }
        }
        "import-step" => {
            let Some(bytes) = bytes else {
                return respond(&id, &op, &key, Err("Missing STEP bytes".into()), None);
            };
            if name.trim().is_empty() {
                return respond(&id, &op, &key, Err("Missing STEP file name".into()), None);
            }
            let file_name = safe_file_name(&name);
            if !matches!(
                ImportFormat::of_file_name(&file_name),
                Some(ImportFormat::Step)
            ) {
                return respond(
                    &id,
                    &op,
                    &key,
                    Err("Expected a .step, .stp or .p21 file".into()),
                    None,
                );
            }
            let data = String::from_utf8_lossy(&bytes).into_owned();
            let pending = cadrs_core::rebuild::exchange::plan_import(
                ImportFormat::Step,
                data.as_bytes().to_vec(),
            );
            world.spawn(StepImport {
                request_id: id,
                document_key: key,
                file_name,
                data,
                pending,
            });
        }
        "save-cadrs" => {
            if !matches_active_key(world, &id, &op, &key) {
                return;
            }
            let store = world.resource::<DocumentStore>().0.clone();
            let user = world.resource::<UserProfile>().id.clone();
            let now = world.resource::<AppClock>().now();
            let (doc, mut meta) = {
                let Some(mut active) = world.get_resource_mut::<ActiveDocument>() else {
                    return respond(&id, &op, &key, Err("No document is open".into()), None);
                };
                active.settle_regeneration();
                (
                    active.doc.clone(),
                    active
                        .meta
                        .clone()
                        .unwrap_or_else(|| DocumentMeta::new(&user, now)),
                )
            };
            meta.modified = now;
            meta.modified_by = user;
            match cadrs_core::native::export_bytes(&doc, &meta, &store) {
                Ok(bytes) => respond(
                    &id,
                    &op,
                    &key,
                    Ok(Some((
                        format!("{}.cadrs", cadrs_core::export::sanitize(&doc.name)),
                        "application/vnd.cadrs".into(),
                        bytes,
                    ))),
                    Some(&doc.name),
                ),
                Err(error) => respond(&id, &op, &key, Err(error), None),
            }
        }
        "export-step" => {
            if !matches_active_key(world, &id, &op, &key) {
                return;
            }
            if let Err(error) =
                crate::export_dialog::start_api_step_export(world, id.clone(), key.clone())
            {
                respond(&id, &op, &key, Err(error), None);
            }
        }
        _ => respond(
            &id,
            &op,
            &key,
            Err(format!("Unknown operation: {op}")),
            None,
        ),
    }
}

fn matches_active_key(world: &World, id: &str, op: &str, key: &str) -> bool {
    let active_id = world
        .get_resource::<ActiveDocument>()
        .map(|active| active.doc.id);
    let registered = world.resource::<ActiveApiDocument>().0.as_ref();
    if registered
        .is_some_and(|(registered_key, doc_id)| registered_key == key && Some(*doc_id) == active_id)
    {
        true
    } else {
        respond(
            id,
            op,
            key,
            Err("documentKey does not identify the active CADRS document".into()),
            None,
        );
        false
    }
}

fn set_active_key(world: &mut World, key: String) {
    if let Some(active) = world.get_resource::<ActiveDocument>() {
        world.resource_mut::<ActiveApiDocument>().0 = Some((key, active.doc.id));
    }
}

fn safe_file_name(name: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .unwrap_or("Import.step")
        .to_string()
}

fn store_new_document(world: &mut World, doc: Document) -> Result<(String, DocumentMeta), String> {
    let store = world.resource::<DocumentStore>().0.clone();
    let user = world.resource::<UserProfile>().id.clone();
    let now = world.resource::<AppClock>().now();
    let mut meta = DocumentMeta::new(&user, now);
    meta.last_opened = Some(now);
    store.create(&doc, &meta).map_err(|e| e.to_string())?;
    let name = doc.name.clone();
    open_document(world, doc, meta.clone());
    Ok((name, meta))
}

fn open_document(world: &mut World, doc: Document, meta: DocumentMeta) {
    let mut active = ActiveDocument::stored(doc, meta);
    active.fresh = true;
    world.insert_resource(active);
    world
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Document);
}

fn finish_step_imports(world: &mut World) {
    let mut query = world.query::<(Entity, &StepImport)>();
    let finished = query
        .iter(world)
        .filter_map(|(entity, import)| import.pending.poll().map(|result| (entity, result)))
        .collect::<Vec<_>>();
    for (entity, result) in finished {
        let Some(import) = world.entity_mut(entity).take::<StepImport>() else {
            continue;
        };
        let plan = match result.unwrap_or_else(|| Err("Kernel stopped during STEP import".into())) {
            Ok(plan) => plan,
            Err(error) => {
                respond(
                    &import.request_id,
                    "import-step",
                    &import.document_key,
                    Err(error),
                    None,
                );
                continue;
            }
        };
        let how = if plan.is_assembly() {
            ImportAs::Assembly
        } else {
            ImportAs::PartStudio
        };
        let doc = imported_document(
            &plan,
            &import.file_name,
            import.data,
            how,
            ImportIds::fresh(),
        );
        match store_new_document(world, doc) {
            Ok((name, _)) => {
                set_active_key(world, import.document_key.clone());
                respond(
                    &import.request_id,
                    "import-step",
                    &import.document_key,
                    Ok(None),
                    Some(&name),
                );
            }
            Err(error) => respond(
                &import.request_id,
                "import-step",
                &import.document_key,
                Err(error),
                None,
            ),
        }
    }
}

pub(crate) fn respond_step_export(
    request_id: &str,
    document_key: &str,
    result: Result<Vec<u8>, String>,
    name: &str,
) {
    match result {
        Ok(bytes) => respond(
            request_id,
            "export-step",
            document_key,
            Ok(Some((name.into(), "model/step".into(), bytes))),
            None,
        ),
        Err(error) => respond(request_id, "export-step", document_key, Err(error), None),
    }
}
