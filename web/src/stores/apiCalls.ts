import { apiCallsApi } from "@/api";
import { createResourceStore } from "./createResourceStore";

export const useApiCallsStore = createResourceStore("apiCalls", apiCallsApi);
