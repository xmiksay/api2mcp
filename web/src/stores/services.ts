import { servicesApi } from "@/api";
import { createResourceStore } from "./createResourceStore";

export const useServicesStore = createResourceStore("services", servicesApi);
