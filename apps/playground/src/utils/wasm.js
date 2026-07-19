import * as tf from '@tensorflow/tfjs';
import { setTfjsRuntime } from '../../../../packages/vite-plugin-waluau/runtime.js';

setTfjsRuntime(tf);

export * from '../../../../packages/vite-plugin-waluau/runtime.js';
