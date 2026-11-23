import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import ShowMode from './ShowMode.svelte';

// The same bundle runs the main window and the fullscreen idle "show" window.
const show = new URLSearchParams(location.search).get('mode') === 'show';

export default mount(show ? ShowMode : App, { target: document.getElementById('app') });
