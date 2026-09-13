import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import express from 'express';
import {createApp} from './app.mjs';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const config=JSON.parse(fs.readFileSync(process.env.PODMESH_WEB_CONFIG||path.join(root,'config.local.json'),'utf8'));
const port=Number(process.env.PORT||4175);const origin=`http://127.0.0.1:${port}`;
const app=createApp(config,{origin});app.use(express.static(path.join(root,'dist')));app.get('/{*path}',(_req,res)=>res.sendFile(path.join(root,'dist/index.html')));app.listen(port,'127.0.0.1',()=>console.log(`PodMesh console: ${origin} (local operator access only)`));
