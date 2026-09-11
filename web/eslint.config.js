import eslint from '@eslint/js'
import pluginVue from 'eslint-plugin-vue'
import { defineConfigWithVueTs, vueTsConfigs } from '@vue/eslint-config-typescript'

export default defineConfigWithVueTs(
  { name: 'app/files-to-lint', files: ['**/*.{ts,mts,tsx,vue}'] },
  { name: 'app/files-to-ignore', ignores: ['**/dist/**', '**/node_modules/**'] },
  eslint.configs.recommended,
  pluginVue.configs['flat/essential'],
  vueTsConfigs.recommended,
  { name: 'app/rules', rules: { 'vue/multi-word-component-names': 'off' } },
)
