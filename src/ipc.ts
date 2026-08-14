import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Annotation, AppSession, AppSnapshot, ChapterNode, DocumentContent, DocumentSummary, MaterialClip, RemoteStatus, SearchQuery, SearchResult, UserSettings } from "./types";

export const isTauri = Boolean(window.__TAURI_INTERNALS__);
export async function call<T>(command:string,args?:Record<string,unknown>):Promise<T>{if(!isTauri) throw new Error("请在 Novalyte 桌面应用中运行");return invoke<T>(command,args)}
export const api = {
  bootstrap:()=>call<AppSnapshot>("bootstrap"),
  snapshot:()=>call<AppSnapshot>("app_snapshot"),
  addLibrary:(path:string)=>call<AppSnapshot>("library_add",{path}),
  removeLibrary:(libraryId:string)=>call<AppSnapshot>("library_remove",{libraryId}),
  refresh:()=>call<AppSnapshot>("library_refresh"),
  read:(documentId:string)=>call<DocumentContent>("document_read",{documentId}),
  tidy:(documentId:string)=>call<DocumentContent>("document_tidy",{documentId}),
  write:(documentId:string,content:string,expectedHash:string)=>call<DocumentContent>("document_write",{input:{documentId,content,expectedHash}}),
  forceWrite:(documentId:string,content:string,expectedHash:string)=>call<DocumentContent>("document_force_write",{input:{documentId,content,expectedHash}}),
  saveAs:(documentId:string,content:string,targetPath:string)=>call<void>("document_save_as",{documentId,content,targetPath}),
  create:(input:object)=>call<AppSnapshot>("document_create",{input}), rename:(input:object)=>call<AppSnapshot>("document_rename",{input}), move:(input:object)=>call<AppSnapshot>("document_move",{input}), trash:(input:object)=>call<AppSnapshot>("document_trash",{input}),
  updateMeta:(input:object)=>call<AppSnapshot>("document_update_meta",{input}),
  shelfUpdate:(documentId:string,shelf:string)=>call<DocumentSummary>("document_shelf",{documentId,shelf}),
  documentPreviews:(ids:string[])=>call<{documentId:string;paragraphs:string[]}[]>("document_previews",{ids}),
  tagUpdate:(documentId:string,tags:string[])=>call<DocumentSummary>("document_tag_update",{documentId,tags}),
  chapterCreate:(input:object)=>call<ChapterNode[]>("chapter_create",{input}), chapterUpdate:(input:object)=>call<ChapterNode[]>("chapter_update",{input}), chapterDelete:(chapterId:string,documentId:string)=>call<ChapterNode[]>("chapter_delete",{chapterId,documentId}),
  saveAnnotation:(input:object)=>call<Annotation[]>("annotation_save",{input}), deleteAnnotation:(annotationId:string,documentId:string)=>call<Annotation[]>("annotation_delete",{annotationId,documentId}), saveMaterial:(input:object)=>call<MaterialClip[]>("material_save",{input}),
  search:(query:SearchQuery)=>call<SearchResult[]>("search",{query}), saveProgress:(input:object)=>call<void>("reading_progress_save",{input}), saveSettings:(settings:UserSettings)=>call<void>("settings_save",{settings}), saveSession:(session:AppSession)=>call<void>("session_save",{session}),
  history:(documentId:string)=>call<unknown[]>("history_list",{documentId}), restoreHistory:(historyId:string,documentId:string)=>call<DocumentContent>("history_restore",{historyId,documentId}),
  remoteStart:()=>call<RemoteStatus>("remote_start"), remoteStop:()=>call<RemoteStatus>("remote_stop"), remoteStatus:()=>call<RemoteStatus>("remote_status"),
  remoteTunnelStart:()=>call<void>("remote_tunnel_start"), remoteTunnelStop:()=>call<void>("remote_tunnel_stop"),
  onPhoneOpen:(handler:(payload:{documentId:string;title:string;deviceName:string})=>void)=>listen<{documentId:string;title:string;deviceName:string}>("remote:phone-open",e=>handler(e.payload)),
  onLibraryChanged:(handler:()=>void)=>listen("library-changed",handler),
};
