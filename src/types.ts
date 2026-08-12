export type Format = "txt" | "epub";
export type Purpose = "原创" | "对标范文" | "素材";
export type Progress = "构思中" | "大纲筹备" | "撰写中" | "连载中" | "待修改" | "已完结" | "全书定稿" | "废稿";
export type Theme = "day" | "protect" | "night" | "parchment";

export interface LibraryRoot { id:string; name:string; documentCount:number }
export interface DocumentSummary { id:string; libraryId:string; relativePath:string; title:string; format:Format; wordCount:number; modifiedAt:number; gender:string; genre:string; subgenre:string; lengthKind:string; purpose:Purpose; progress:Progress; favorite:boolean; missing:boolean; tags:string[] }
export interface TreeNode { name:string; relativePath:string; kind:"library"|"folder"|"document"; libraryId:string; documentId?:string; count:number; children:TreeNode[] }
export interface ChapterNode { id:string; documentId:string; title:string; offset:number; kind:"auto"|"manual"; level:number }
export interface Annotation { id:string; documentId:string; startOffset:number; endOffset:number; quote:string; prefix:string; suffix:string; note:string; marker:string; orphaned:boolean; createdAt:number }
export interface MaterialClip { id:string; documentId:string; quote:string; note:string; groupName:string; sourceTitle:string; startOffset:number; createdAt:number }
export interface VirtualGroup { id:string; name:string; documentIds:string[] }
export interface ReadingProgress { documentId:string; chapterId?:string; charOffset:number; scrollRatio:number }
export interface ReaderSettings { theme:Theme; fontFamily:string; fontSize:number; letterSpacing:number; lineHeight:number; paperWidth:number; pageMargin:number; speechRate:number }
export interface UserSettings { reader:ReaderSettings; shortcuts:Record<string,string>; alwaysOnTop:boolean }
export interface AppSession { leftDocumentId?:string; rightDocumentId?:string; split:boolean; splitRatio:number; sidebarOpen:boolean; detailOpen:boolean; activeLibraryId?:string }
export interface AppSnapshot { libraries:LibraryRoot[]; documents:DocumentSummary[]; tree:TreeNode[]; groups:VirtualGroup[]; materials:MaterialClip[]; settings:UserSettings; session:AppSession }
export interface DocumentContent { summary:DocumentSummary; content:string; contentHash:string; encoding:string; newline:string; editable:boolean; chapters:ChapterNode[]; annotations:Annotation[]; readingProgress?:ReadingProgress }
export interface SearchQuery { text:string; tag?:string; libraryId?:string; lengthKind?:string; purpose?:Purpose; progress?:Progress; format?:Format }
export interface SearchResult { document:DocumentSummary; snippet:string }

declare global { interface Window { __TAURI_INTERNALS__?: unknown } }
