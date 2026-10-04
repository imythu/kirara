import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** 页面内切换到详情等子视图时回到内容区顶部（桌面端内容区自身滚动，移动端为窗口滚动）。 */
export function scrollContentToTop() {
  document.querySelector<HTMLElement>(".kirara-content")?.scrollTo({ top: 0 });
  window.scrollTo({ top: 0 });
}
