import { scriptsApi } from "@/api";
import { createResourceStore } from "./createResourceStore";

export const useScriptsStore = createResourceStore("scripts", scriptsApi);
