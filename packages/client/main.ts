import ui from '@nuxt/ui/vue-plugin'
import { createPinia } from 'pinia'
// register vue composition api globally
import { createApp } from 'vue'
import App from './App.vue'

// tailwind css
import './index.css'

const app = createApp(App)
const pinia = createPinia()

app.use(pinia)
app.use(ui)
app.mount('#app')
