class WaluauCompilerClient {
  constructor() {
    this.worker = new Worker(new URL('../workers/waluauCompiler.worker.js', import.meta.url), {
      type: 'module',
    });
    this.nextRequestId = 0;
    this.requests = new Map();
    this.analysisInFlight = null;
    this.pendingAnalysis = null;
    this.analysisIdleWaiters = [];
    this.initializePromise = null;
    this.failure = null;

    this.worker.onmessage = ({ data }) => {
      const request = this.requests.get(data.id);
      if (!request) return;
      this.requests.delete(data.id);
      if (data.ok) {
        request.resolve(data.result);
      } else {
        request.reject(new Error(data.error));
      }
    };
    this.worker.onerror = (event) => {
      const error = new Error(event.message || 'Waluau compiler worker failed');
      this.failure = error;
      for (const request of this.requests.values()) {
        request.reject(error);
      }
      this.requests.clear();
      this.pendingAnalysis?.reject(error);
      this.pendingAnalysis = null;
      for (const resolve of this.analysisIdleWaiters.splice(0)) {
        resolve();
      }
    };
  }

  request(type, payload) {
    if (this.failure) {
      return Promise.reject(this.failure);
    }
    const id = ++this.nextRequestId;
    return new Promise((resolve, reject) => {
      this.requests.set(id, { resolve, reject });
      this.worker.postMessage({ id, type, payload });
    });
  }

  initialize() {
    if (!this.initializePromise) {
      this.initializePromise = this.request('initialize', {});
    }
    return this.initializePromise;
  }

  /**
   * Analyze and compile a complete project snapshot. While one snapshot is
   * running, repeated edits collapse into one latest pending snapshot instead
   * of building an unbounded queue of obsolete whole-project work.
   * Superseded calls resolve to null.
   */
  analyzeProject(files, entryFile) {
    if (
      this.analysisInFlight?.files === files &&
      this.analysisInFlight?.entryFile === entryFile
    ) {
      return this.analysisInFlight.promise;
    }
    if (this.pendingAnalysis?.files === files && this.pendingAnalysis?.entryFile === entryFile) {
      return this.pendingAnalysis.promise;
    }

    let resolveAnalysis;
    let rejectAnalysis;
    const promise = new Promise((resolve, reject) => {
      resolveAnalysis = resolve;
      rejectAnalysis = reject;
    });
    const analysis = {
      files,
      entryFile,
      promise,
      resolve: resolveAnalysis,
      reject: rejectAnalysis,
    };
    if (this.analysisInFlight) {
      this.pendingAnalysis?.resolve(null);
      this.pendingAnalysis = analysis;
    } else {
      this.startAnalysis(analysis);
    }
    return promise;
  }

  startAnalysis(analysis) {
    this.analysisInFlight = analysis;
    this.request('analyzeProject', {
      files: analysis.files,
      entryFile: analysis.entryFile,
    })
      .then(analysis.resolve, analysis.reject)
      .finally(() => {
        this.analysisInFlight = null;
        const pending = this.pendingAnalysis;
        this.pendingAnalysis = null;
        if (pending) {
          this.startAnalysis(pending);
          return;
        }
        for (const resolve of this.analysisIdleWaiters.splice(0)) {
          resolve();
        }
      });
  }

  whenAnalysisIdle() {
    if (!this.analysisInFlight && !this.pendingAnalysis) {
      return Promise.resolve();
    }
    return new Promise((resolve) => this.analysisIdleWaiters.push(resolve));
  }

  async lspRequest(method, params, document = null) {
    await this.whenAnalysisIdle();
    return this.request('lspRequest', { method, params, document });
  }

  compileProject(files, entryFile) {
    return this.request('compileProject', { files, entryFile });
  }
}

let sharedClient = null;

export function getWaluauCompilerClient() {
  if (!sharedClient) {
    sharedClient = new WaluauCompilerClient();
  }
  return sharedClient;
}
