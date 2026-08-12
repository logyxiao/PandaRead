import { Component, type ErrorInfo, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { Toaster } from "sonner";
import App from "./App";
import "./styles.css";

class AppErrorBoundary extends Component<{children:ReactNode},{error?:Error}>{
  state:{error?:Error}={};
  static getDerivedStateFromError(error:Error){return {error}}
  componentDidCatch(error:Error,info:ErrorInfo){console.error("Novalyte render failed",error,info)}
  render(){if(this.state.error)return <div className="boot-screen"><strong>Novalyte 页面加载失败</strong><span>{this.state.error.message||"界面运行时发生错误"}</span><button onClick={()=>location.reload()}>重新打开</button></div>;return this.props.children}
}

createRoot(document.getElementById("root")!).render(<AppErrorBoundary><App /><Toaster position="bottom-right" richColors /></AppErrorBoundary>);
