import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router'

// Login and OAuth consent are server-rendered, so they are deliberately absent here: the SPA
// has no auth views at all and a 401 is answered by redirecting to /login.
const routes: RouteRecordRaw[] = [
  { path: '/', name: 'home', component: () => import('@/views/HomeView.vue') },
]

export const router = createRouter({ history: createWebHistory(), routes })
