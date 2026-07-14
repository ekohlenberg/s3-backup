using System;
using System.Collections.Generic;
using System.Text;
using System.Configuration;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Configuration.Json;
using System.IO;

namespace s3b
{
    public class Config : Model
    {
        static IConfiguration configBuilder = null;

        protected static Config _config = null;
        protected Template _template = null;

        private IConfiguration getConfigBuilder()
        {
            if (configBuilder == null)
            {
                configBuilder = new ConfigurationBuilder()
                .AddJsonFile("appsettings.json", true, true)
                .Build();
            }

            return configBuilder;
        }
         public string getString(string k)
        {
            
            string result = string.Empty;

            if (ContainsKey(k))
            {
                object o = this[k];

                if (o != null)
                {
                    result = o.ToString();

                    Template t = getTemplate();

                    result = t.eval(result, this);
                }
            }

            return result;
        }

        private string getConfigString(string param)
        {
            IConfigurationSection section = getConfigBuilder().GetSection("appsettings");

            return section[param];
        }

        protected  Template getTemplate()
        {
            if (_template == null)
            {
                _template = new Template();
            }

            return _template;
        }

         public void setValue(string k, string v)
        {
            if (ContainsKey(k))
            {
                this[k] = v;
            }
            else
            {
                Add(k, v);
            }
        }

        public void setValue(string k, int v)
        {
            if (ContainsKey(k))
            {
                this[k] = v.ToString();
            }
            else
            {
                Add(k, v.ToString());
            }
        }


        public int getInt(string k)
        {
            int result = 0;

            if (ContainsKey(k))
            {
                result = Convert.ToInt32(this[k]);
            }

            return result;
        }

        static public Config getConfig()
        {
            if (_config == null)
            {
                _config = new Config();

                IConfigurationSection section = _config.getConfigBuilder().GetSection("appsettings");

                foreach (var c in section.GetChildren())
                {
                    string k = c.Key;
                    string v = _config.getConfigString(k);

                    _config.Add(k, v);
                }

                string passfile = System.Environment.GetEnvironmentVariable("S3BPASSFILE");
                if (passfile == null)
                    passfile = System.Environment.GetEnvironmentVariable("S3B-PASSFILE");
#if DEBUG
                if (passfile == null)
                    passfile = "/Library/s3b/data/id_pass";
#endif
                if (passfile == null) throw new Exception("S3B-PASSFILE or S3BPASSFILE not defined.");

                if (!File.Exists(passfile)) throw new Exception("Password file not found.");
                _config.setValue("passfile", passfile);
            }

            return _config;
        }
    }
}
