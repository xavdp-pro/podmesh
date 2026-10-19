module.exports = {
  apps: [
    {
      name: 'podmesh-vitrine',
      script: 'server.js',
      cwd: __dirname,
      env: {
        NODE_ENV: 'production',
        PORT: 7620,
        HOST: '0.0.0.0',
      },
      autorestart: true,
      max_restarts: 10,
    },
  ],
}
