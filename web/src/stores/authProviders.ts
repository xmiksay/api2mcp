import { authProvidersApi } from "@/api";
import { createResourceStore } from "./createResourceStore";

export const useAuthProvidersStore = createResourceStore("authProviders", authProvidersApi);
