import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router'

// Login and OAuth consent are server-rendered, so they are deliberately absent here: the SPA
// has no auth views at all and a 401 is answered by redirecting to /login (see api/client.ts).
//
// Every route below must have a nav entry in components/NavSidebar.vue — no orphan routes.
const routes: RouteRecordRaw[] = [
  { path: '/', name: 'home', component: () => import('@/views/HomeView.vue') },

  { path: '/services', name: 'services', component: () => import('@/views/ServicesListView.vue') },
  {
    path: '/services/new',
    name: 'service-new',
    component: () => import('@/views/ServiceFormView.vue'),
    props: { mode: 'create' },
  },
  {
    path: '/services/:slug',
    name: 'service-detail',
    component: () => import('@/views/ServiceDetailView.vue'),
    props: true,
  },
  {
    path: '/services/:slug/edit',
    name: 'service-edit',
    component: () => import('@/views/ServiceFormView.vue'),
    props: (route) => ({ mode: 'edit', slug: route.params.slug }),
  },

  {
    path: '/auth-providers',
    name: 'auth-providers',
    component: () => import('@/views/AuthProvidersListView.vue'),
  },
  {
    path: '/auth-providers/new',
    name: 'auth-provider-new',
    component: () => import('@/views/AuthProviderFormView.vue'),
    props: { mode: 'create' },
  },
  {
    path: '/auth-providers/:slug',
    name: 'auth-provider-detail',
    component: () => import('@/views/AuthProviderDetailView.vue'),
    props: true,
  },
  {
    path: '/auth-providers/:slug/edit',
    name: 'auth-provider-edit',
    component: () => import('@/views/AuthProviderFormView.vue'),
    props: (route) => ({ mode: 'edit', slug: route.params.slug }),
  },

  { path: '/api-calls', name: 'api-calls', component: () => import('@/views/ApiCallsListView.vue') },
  {
    path: '/api-calls/new',
    name: 'api-call-new',
    component: () => import('@/views/ApiCallFormView.vue'),
    props: { mode: 'create' },
  },
  {
    path: '/api-calls/:slug',
    name: 'api-call-detail',
    component: () => import('@/views/ApiCallDetailView.vue'),
    props: true,
  },
  {
    path: '/api-calls/:slug/edit',
    name: 'api-call-edit',
    component: () => import('@/views/ApiCallFormView.vue'),
    props: (route) => ({ mode: 'edit', slug: route.params.slug }),
  },

  { path: '/scripts', name: 'scripts', component: () => import('@/views/ScriptsListView.vue') },
  {
    path: '/scripts/new',
    name: 'script-new',
    component: () => import('@/views/ScriptFormView.vue'),
    props: { mode: 'create' },
  },
  {
    path: '/scripts/:slug',
    name: 'script-detail',
    component: () => import('@/views/ScriptDetailView.vue'),
    props: true,
  },
  {
    path: '/scripts/:slug/edit',
    name: 'script-edit',
    component: () => import('@/views/ScriptFormView.vue'),
    props: (route) => ({ mode: 'edit', slug: route.params.slug }),
  },

  { path: '/endpoints', name: 'endpoints', component: () => import('@/views/EndpointsListView.vue') },
  {
    path: '/endpoints/new',
    name: 'endpoint-new',
    component: () => import('@/views/EndpointFormView.vue'),
    props: { mode: 'create' },
  },
  {
    path: '/endpoints/:slug',
    name: 'endpoint-detail',
    component: () => import('@/views/EndpointDetailView.vue'),
    props: true,
  },
  {
    path: '/endpoints/:slug/edit',
    name: 'endpoint-edit',
    component: () => import('@/views/EndpointFormView.vue'),
    props: (route) => ({ mode: 'edit', slug: route.params.slug }),
  },
  {
    path: '/endpoints/:slug/plan',
    name: 'endpoint-plan',
    component: () => import('@/views/EndpointPlanView.vue'),
    props: true,
  },

  { path: '/runs', name: 'runs', component: () => import('@/views/RunsView.vue') },
  {
    path: '/runs/:id',
    name: 'run-detail',
    component: () => import('@/views/RunDetailView.vue'),
    props: true,
  },

  { path: '/health', name: 'health', component: () => import('@/views/HealthView.vue') },

  { path: '/tokens', name: 'tokens', component: () => import('@/views/TokensListView.vue') },
]

export const router = createRouter({ history: createWebHistory(), routes })
