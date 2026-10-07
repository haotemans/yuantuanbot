import { createRouter, createWebHistory } from 'vue-router'
import { isLoggedIn } from '../api'

/** @type {import('vue-router').RouteRecordRaw[]} */
const routes = [
  { path: '/login', name: 'login', component: () => import('../views/Login.vue') },
  { path: '/', name: 'dashboard', component: () => import('../views/Dashboard.vue') },
  { path: '/trace', name: 'trace', component: () => import('../views/Trace.vue') },
  { path: '/tasks', name: 'tasks', component: () => import('../views/Tasks.vue') },
  { path: '/memories', name: 'memories', component: () => import('../views/Memories.vue') },
  { path: '/relations', name: 'relations', component: () => import('../views/Relations.vue') },
  { path: '/platform', name: 'platform', component: () => import('../views/Platform.vue') },
  { path: '/models', name: 'models', component: () => import('../views/Models.vue') },
  { path: '/params', name: 'params', component: () => import('../views/Params.vue') },
  { path: '/personality', name: 'personality', component: () => import('../views/Personality.vue') },
  { path: '/meme', name: 'meme', component: () => import('../views/Meme.vue') },
  { path: '/kb', name: 'kb', component: () => import('../views/Kb.vue') },
  { path: '/plugins', name: 'plugins', component: () => import('../views/Plugins.vue') },
  { path: '/mcp', name: 'mcp', component: () => import('../views/Mcp.vue') },
  { path: '/backup', name: 'backup', component: () => import('../views/Backup.vue') },
  { path: '/:pathMatch(.*)*', redirect: '/' },
]

const router = createRouter({ history: createWebHistory(), routes })

router.beforeEach((to) => {
  if (to.name !== 'login' && !isLoggedIn()) return { name: 'login' }
  if (to.name === 'login' && isLoggedIn()) return { name: 'dashboard' }
})

export default router
